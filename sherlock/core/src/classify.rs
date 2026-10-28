//! The confidence model the pipeline derives from its own evidence.
//!
//! `sherlock_app_lib::classify` owns the Ollama client, the prompts and the parsing.
//! What is shared here is the number that ends up in `files.confidence` and the list of
//! media types that number is predicated on: a reader of the index has to interpret
//! those values the same way the writer produced them.

/// Media types the pipeline accepts from the model.
///
/// Anything else is folded into `other` before it reaches the database, so this list is
/// also the vocabulary a reader can rely on.
pub const VALID_MEDIA_TYPES: &[&str] = &[
    "screenshot",
    "anime",
    "manga",
    "photo",
    "document",
    "artwork",
    "other",
];

/// How the first pass arrived at its answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParseKind {
    /// Valid JSON with a `media_type` on the first attempt.
    Strict,
    /// Recovered from the body text by the salvage pass.
    Salvaged,
    /// Nothing usable came back; the value is the safe default object.
    Default,
}

/// A confidence number the **pipeline** can justify.
///
/// The model's own `confidence` field was removed from the primary schema after job #35
/// (2026-10-28) measured **22,809 of 36,744** described rows at exactly 0.0: the schema
/// showed `"confidence":0.0` and the model copied that literal at temperature 0. A number
/// the model supplies cannot measure the model's own certainty, so the score is derived
/// from evidence that exists outside the answer:
///
/// - how the answer was obtained (strict JSON > salvage);
/// - whether `media_type` came back as one of the values we accept;
/// - whether the description has a usable length;
/// - whether a second signal corroborated it (OCR text, or a confirmed document kind).
///
/// `Default` returns exactly 0.0, which is what keeps "is this row classified" honest: a
/// row with no description must stay pending, not be stored with a small score.
///
/// The ceiling is 0.95 on purpose — nothing the model says is certain enough for 1.0.
pub fn derived_confidence(
    parse: ParseKind,
    media_type: &str,
    description: &str,
    corroborated: bool,
) -> f32 {
    let mut score: f32 = match parse {
        ParseKind::Strict => 0.80,
        ParseKind::Salvaged => 0.55,
        ParseKind::Default => return 0.0,
    };
    if media_type == "other" || !VALID_MEDIA_TYPES.contains(&media_type) {
        score -= 0.15;
    }
    let words = description.split_whitespace().count();
    if !(3..=24).contains(&words) {
        score -= 0.10;
    }
    if corroborated {
        score += 0.10;
    }
    score.clamp(0.05, 0.95)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_answer_scores_zero() {
        assert_eq!(
            derived_confidence(ParseKind::Default, "photo", "a dog", true),
            0.0
        );
    }

    #[test]
    fn strict_beats_salvaged() {
        let strict = derived_confidence(ParseKind::Strict, "photo", "a dog on a beach", false);
        let salvaged = derived_confidence(ParseKind::Salvaged, "photo", "a dog on a beach", false);
        assert!(strict > salvaged);
    }

    #[test]
    fn unknown_media_type_costs() {
        let known = derived_confidence(ParseKind::Strict, "photo", "a dog on a beach", false);
        let unknown = derived_confidence(ParseKind::Strict, "blob", "a dog on a beach", false);
        assert!(unknown < known);
    }

    #[test]
    fn corroboration_helps_and_the_ceiling_holds() {
        let plain = derived_confidence(ParseKind::Strict, "photo", "a dog on a beach", false);
        let corroborated = derived_confidence(ParseKind::Strict, "photo", "a dog on a beach", true);
        assert!(corroborated > plain);
        assert!(corroborated <= 0.95);
    }
}
