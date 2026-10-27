//! Rewrite descriptions that are already in the index into the configured language.
//!
//! `description_language` only affects what the classifier writes from now on, and the
//! index is mostly older than that: on the machine this was written for, 3,664 rows had
//! a description and 55,079 did not, so the honest conversion is a **text-only** pass
//! over the classified rows — no image read, no thumbnail, no GPU prefill, just one
//! short generation per row (measured: ~0.15 s each at 768 px settings).
//!
//! Re-describing the image with the vision model would be more faithful (PT-BR at 3b is
//! terser than English, see [`crate::config::DescriptionLanguage`]) but it costs a full
//! pipeline pass, and it would throw away OCR text and mentions to gain nothing that the
//! user asked for. Translating keeps the analysis that was already paid for.
//!
//! Two kinds of text are never translated:
//! - the machine markers in `[brackets]` (`[Doc: …]`, `[Series: …]`, the video
//!   `[10m 20s 1280x720]` prefix), which are matched in Rust and searched verbatim;
//! - [`PROTECTED_PDF_MARKER`], which `db::list_protected_pdfs` compares in SQL.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::config::DescriptionLanguage;
use crate::db::{self, DescriptionRow};
use crate::error::AppResult;
use crate::llm::{num_ctx_for, ollama_generate, GenerateParams};

/// The one description the app writes as a *marker* rather than as prose.
///
/// `db::list_protected_pdfs` selects on `description = 'Password-protected PDF
/// (skipped)'`, so a translated copy would silently empty the PDF password screen.
pub const PROTECTED_PDF_MARKER: &str = "Password-protected PDF (skipped)";

/// A short translation never needs more than this; the vision pass reserves the same
/// window, so a conversion run does not force Ollama to rebuild the runner.
const NUM_PREDICT: u32 = 200;

/// How many descriptions are converted before the row is skipped. A translation that
/// triples the text is not a translation.
const MAX_GROWTH_CHARS: usize = 60;

#[derive(Debug, Default, Clone)]
pub struct ConversionSummary {
    pub total: u64,
    pub processed: u64,
    pub converted: u64,
    pub skipped: u64,
    pub failed: u64,
}

enum RowOutcome {
    Converted(String),
    Skipped,
    Failed,
}

/// Whether a row still needs this pass.
///
/// `description_lang` empty means "written before the column existed", so those rows
/// are picked up — that is exactly the existing English index. A row that already
/// carries the target tag is left alone, which is what makes the pass resumable and
/// re-runnable.
pub fn needs_conversion(row: &DescriptionRow, target: DescriptionLanguage) -> bool {
    if row.description.trim().is_empty() {
        return false;
    }
    if target.owns_tag(&row.description_lang) {
        return false;
    }
    !is_machine_marker(row)
}

fn is_machine_marker(row: &DescriptionRow) -> bool {
    row.media_type == "document" && row.description.trim() == PROTECTED_PDF_MARKER
}

/// Split a description into `(leading markers, prose, trailing markers)`.
///
/// The classifier only ever puts bracketed spans at the ends: it appends
/// `[Series: …] [Doc: …] [Issuer: …]` and prefixes the video frame info
/// (`[10m 20s 1280x720]`). Those spans are matched in Rust and searched verbatim, so
/// they are **never sent to the model**. Two measured reasons:
/// - placeholders (`⟦0⟧`) are dropped by qwen2.5-VL: 7 of 8 real rows came back
///   without them, which made the whole conversion unusable;
/// - leaving the brackets in the prompt makes it translate their content —
///   `[Doc: bank_transfer]` became `[Documento: transferência bancária]`, which breaks
///   `document_kind_of` and the document bucket.
///
/// Slicing them off and putting the original bytes back is exact, and 10 of 10 real
/// rows kept every marker.
fn split_markers(text: &str) -> (&str, &str, &str) {
    let mut lead_end = 0;
    let mut rest = text;
    while rest.starts_with('[') {
        let Some(close) = rest.find(']') else { break };
        lead_end += close + 1;
        rest = &rest[close + 1..];
    }

    let mut trail_start = text.len();
    loop {
        let candidate = text[..trail_start].trim_end();
        if !candidate.ends_with(']') {
            break;
        }
        let Some(open) = candidate.rfind('[') else {
            break;
        };
        if open < lead_end {
            break;
        }
        trail_start = open;
    }

    (
        &text[..lead_end],
        &text[lead_end..trail_start],
        &text[trail_start..],
    )
}

/// Put the markers back around the translated prose.
///
/// The prose arrives trimmed, so the separator the original had is restored here: the
/// video prefix and the appended markers are always separated by one space.
fn join_markers(lead: &str, prose: &str, trail: &str) -> String {
    let mut out = String::with_capacity(lead.len() + prose.len() + trail.len() + 2);
    out.push_str(lead);
    if !lead.is_empty() && !lead.ends_with(char::is_whitespace) && !prose.is_empty() {
        out.push(' ');
    }
    out.push_str(prose);
    if !trail.is_empty() && !trail.starts_with(char::is_whitespace) && !prose.is_empty() {
        out.push(' ');
    }
    out.push_str(trail);
    out
}

fn translate_prompt(language: DescriptionLanguage, text: &str) -> String {
    match language {
        DescriptionLanguage::PtBr => format!(
            "Traduza para português do Brasil o texto abaixo, que descreve uma foto, um \
             vídeo ou um documento. Mantenha exatamente a mesma informação e o mesmo nível \
             de detalhe, sem acrescentar nada. Responda apenas com a tradução, em uma \
             linha, sem aspas.\n\nTEXTO: {text}"
        ),
        DescriptionLanguage::En => format!(
            "Translate the text below, which describes a photo, a video or a document, into \
             English. Keep exactly the same information and level of detail and add nothing. \
             Reply with the translation only, on one line, without quotes.\n\nTEXT: {text}"
        ),
    }
}

/// Pull a usable single line out of a free-form answer.
fn clean_translation(raw: &str) -> String {
    let line = raw
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("");
    let line = line.trim_start_matches(['-', '*', '•']).trim();
    let line = line
        .trim_matches(['"', '\'', '`', '“', '”', '‘', '’'])
        .trim();
    line.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn convert_row(model: &str, target: DescriptionLanguage, row: &DescriptionRow) -> RowOutcome {
    let source = row.description.trim();
    let (lead, prose, trail) = split_markers(source);
    let prose = prose.trim();
    if prose.is_empty() {
        // Nothing but markers: there is no language to change.
        return RowOutcome::Skipped;
    }
    let prompt = translate_prompt(target, prose);
    let num_ctx = num_ctx_for(0, 0, prompt.len(), NUM_PREDICT);
    let params = GenerateParams {
        json_mode: false,
        ..GenerateParams::vision(NUM_PREDICT, Some(num_ctx))
    };
    let resp = ollama_generate(model, &prompt, None, &params);
    if !resp.ok {
        log::warn!(
            "description conversion: request failed for row {} ({}): {}",
            row.id,
            row.description_lang,
            crate::classify::truncate_chars(&resp.raw, 200)
        );
        return RowOutcome::Failed;
    }

    let cleaned = clean_translation(&resp.raw);
    if cleaned.is_empty() {
        return RowOutcome::Skipped;
    }
    let restored = join_markers(lead, &cleaned, trail);
    if restored.chars().count() > source.chars().count() * 3 + MAX_GROWTH_CHARS {
        log::warn!(
            "description conversion: row {} grew from {} to {} chars, keeping the original",
            row.id,
            source.chars().count(),
            restored.chars().count()
        );
        return RowOutcome::Failed;
    }
    if restored == source {
        // Nothing changed: the model echoed the input. Writing the tag would lie about
        // the language of the row, so it stays untagged and is retried next time.
        return RowOutcome::Skipped;
    }
    RowOutcome::Converted(restored)
}

/// Convert every described row that is not yet in `target`.
///
/// Sequential on purpose: each row is one small generation, Ollama serves them one at a
/// time anyway, and a single thread keeps the progress counter honest.
pub fn convert_existing_descriptions(
    db_path: &Path,
    model: &str,
    target: DescriptionLanguage,
    cancel: &AtomicBool,
    on_progress: impl Fn(&ConversionSummary),
) -> AppResult<ConversionSummary> {
    let rows = db::list_descriptions_needing_language(db_path, target.as_str())?;
    let mut summary = ConversionSummary {
        total: rows.len() as u64,
        ..Default::default()
    };
    log::info!(
        "description conversion: {} row(s) to convert into {}",
        summary.total,
        target.as_str()
    );

    for row in &rows {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        // The work list is "described but not in this language"; the marker that the
        // SQL compares (`PROTECTED_PDF_MARKER`) is the only row that must stay as it is.
        if !needs_conversion(row, target) {
            summary.skipped += 1;
            summary.processed += 1;
            continue;
        }
        match convert_row(model, target, row) {
            RowOutcome::Converted(text) => {
                match db::set_file_description(db_path, row.id, &text, target.as_str()) {
                    Ok(()) => summary.converted += 1,
                    Err(e) => {
                        log::warn!(
                            "description conversion: could not store row {}: {e}",
                            row.id
                        );
                        summary.failed += 1;
                    }
                }
            }
            RowOutcome::Skipped => summary.skipped += 1,
            RowOutcome::Failed => summary.failed += 1,
        }
        summary.processed += 1;
        if summary.processed % 10 == 0 || summary.processed == summary.total {
            on_progress(&summary);
        }
    }
    // A cancelled run stops mid-list: report the real counters once more so the UI does
    // not show the last multiple-of-ten.
    on_progress(&summary);
    Ok(summary)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(description: &str, lang: &str) -> DescriptionRow {
        DescriptionRow {
            id: 1,
            media_type: "photo".to_string(),
            description: description.to_string(),
            description_lang: lang.to_string(),
        }
    }

    #[test]
    fn trailing_markers_never_reach_the_model() {
        // The real shape the classifier writes: prose followed by appended markers.
        let original = "group of people walking on a road [Doc: bank_transfer] [Issuer: Bank Name]";
        let (lead, prose, trail) = split_markers(original);
        assert_eq!(lead, "");
        assert_eq!(prose.trim(), "group of people walking on a road");
        assert_eq!(trail, "[Doc: bank_transfer] [Issuer: Bank Name]");
        assert_eq!(
            join_markers(lead, "grupo de pessoas andando na rua", trail),
            "grupo de pessoas andando na rua [Doc: bank_transfer] [Issuer: Bank Name]"
        );
    }

    #[test]
    fn the_video_prefix_is_kept_and_the_category_restored() {
        // "[duration resolution] Title: description [category] [Series: …]"
        let original = "[11m 48s 1280x720] Olimpíadas 2016: grupo de atletas [personal]";
        let (lead, prose, trail) = split_markers(original);
        assert_eq!(lead, "[11m 48s 1280x720]");
        assert_eq!(prose.trim(), "Olimpíadas 2016: grupo de atletas");
        assert_eq!(trail, "[personal]");
        assert_eq!(
            join_markers(lead, "grupo de atletas em pista", trail),
            "[11m 48s 1280x720] grupo de atletas em pista [personal]"
        );
    }

    #[test]
    fn text_without_markers_is_all_prose() {
        let (lead, prose, trail) = split_markers("a sunny beach photo");
        assert_eq!(lead, "");
        assert_eq!(prose, "a sunny beach photo");
        assert_eq!(trail, "");
    }

    #[test]
    fn an_unbalanced_bracket_stays_in_the_prose() {
        let (lead, prose, trail) = split_markers(" grupo [incompleto na praia");
        assert_eq!(lead, "");
        assert_eq!(prose, " grupo [incompleto na praia");
        assert_eq!(trail, "");
    }

    #[test]
    fn markers_only_leave_no_prose() {
        let (lead, prose, trail) = split_markers("[Doc: receipt] [Issuer: Bank]");
        assert_eq!(lead, "[Doc: receipt]");
        assert_eq!(prose.trim(), "");
        assert_eq!(trail, "[Issuer: Bank]");
    }

    #[test]
    fn clean_translation_takes_the_first_real_line() {
        assert_eq!(
            clean_translation("\n\n  \"uma foto de praia\"\n"),
            "uma foto de praia"
        );
        assert_eq!(clean_translation("- duas   pessoas\nlixo"), "duas pessoas");
    }

    #[test]
    fn the_pdf_marker_is_never_converted() {
        let mut marker = row(PROTECTED_PDF_MARKER, "");
        marker.media_type = "document".to_string();
        assert!(!needs_conversion(&marker, DescriptionLanguage::PtBr));
        assert!(
            !needs_conversion(&marker, DescriptionLanguage::En),
            "the marker must stay byte-identical, the SQL compares it"
        );
    }

    #[test]
    fn rows_already_in_the_target_language_are_left_alone() {
        assert!(needs_conversion(
            &row("homem na praia", ""),
            DescriptionLanguage::PtBr
        ));
        assert!(!needs_conversion(
            &row("homem na praia", "pt_br"),
            DescriptionLanguage::PtBr
        ));
        assert!(needs_conversion(
            &row("man on the beach", "en"),
            DescriptionLanguage::PtBr
        ));
        assert!(
            needs_conversion(&row("homem na praia", "pt_br"), DescriptionLanguage::En),
            "flipping the setting back converts the Portuguese rows, not the English ones"
        );
        assert!(!needs_conversion(
            &row("man on the beach", "en"),
            DescriptionLanguage::En
        ));
        assert!(
            !needs_conversion(&row("", ""), DescriptionLanguage::PtBr),
            "an unclassified row is the classifier's job, not this pass"
        );
    }
}
