import { useEffect, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import {
  cancelDescriptionConversion,
  cancelPublish,
  convertExistingDescriptions,
  getDescriptionLanguageStatus,
  getFaceThreadPlan,
  getPublishLastReport,
  getPublishStatus,
  getVisionSettings,
  loadUserConfig,
  publishLibrary,
  saveUserConfig,
  updateSettings,
} from "../../api";
import type {
  DescriptionLanguageProgress,
  DescriptionLanguageSetting,
  FaceThreadPlan,
  ImageFilterSetting,
  PublishProgress,
  PublishReport,
  SettingsUpdatePayload,
  StartupScanSetting,
  VisionOcrEngine,
  VisionPromptProfile,
  VisionSettings,
} from "../../types";
import { formatBytes } from "../../utils/format";
import ModalOverlay from "./ModalOverlay";
import "./shared-modal.css";
import "./SettingsModal.css";

type Props = {
  onClose: () => void;
  /** Called with what the backend saved, so the caller can refresh its state. */
  onSaved?: (settings: VisionSettings) => void;
  /** Opens the live log viewer on top of this modal. */
  onOpenLogs?: () => void;
};

/**
 * Deployment settings for the web app, stored in `config.json` rather than
 * `settings.toml`: where the snapshot goes describes *this machine's* share, not how
 * the pipeline classifies images. They also save on their own, because the publish
 * button has to act on the value that was persisted, not on what is on screen.
 */
export type PublishConfig = {
  dir: string;
  auto: boolean;
  includeFaceCrops: boolean;
  keepRevisions: number;
};

export const DEFAULT_PUBLISH: PublishConfig = {
  dir: "",
  auto: true,
  includeFaceCrops: true,
  keepRevisions: 2,
};

/** Read the `publish` block out of `config.json`, tolerating a missing/partial one. */
export function readPublishConfig(config: Record<string, unknown>): PublishConfig {
  const raw = (config.publish ?? {}) as Partial<PublishConfig>;
  return {
    dir: typeof raw.dir === "string" ? raw.dir : "",
    // `!== false` keeps the plan's default (on) while still honouring an explicit off.
    auto: raw.auto !== false,
    includeFaceCrops: raw.includeFaceCrops !== false,
    keepRevisions:
      typeof raw.keepRevisions === "number" && raw.keepRevisions >= 1
        ? Math.min(10, Math.round(raw.keepRevisions))
        : DEFAULT_PUBLISH.keepRevisions,
  };
}

const PROMPT_PROFILES: { value: VisionPromptProfile; label: string }[] = [
  { value: "concise", label: "Concise (fewer output tokens)" },
  { value: "detailed", label: "Detailed (adds anime candidates)" },
];

const IMAGE_FILTERS: { value: ImageFilterSetting; label: string; hint: string }[] = [
  { value: "auto", label: "Auto", hint: "box for JPEG/HEIC, Lanczos3 for PNG/BMP/TIFF" },
  { value: "box", label: "Box", hint: "fastest; measured −33 % on the downscale" },
  { value: "lanczos", label: "Lanczos3", hint: "sharpest text, three times slower" },
];

const DESCRIPTION_LANGUAGES: { value: DescriptionLanguageSetting; label: string }[] = [
  { value: "pt_br", label: "Português (Brasil)" },
  { value: "en", label: "English" },
];

const OCR_ENGINES: { value: VisionOcrEngine; label: string }[] = [
  { value: "llm", label: "LLM (multimodal model)" },
  { value: "surya", label: "Surya (isolated venv)" },
  { value: "off", label: "Off" },
];

/** Library profile options; decides whether the anime enrichment pass runs. */
const LIBRARY_KINDS: { value: "auto" | "photos" | "anime"; label: string }[] = [
  { value: "auto", label: "Automatic (decided per file)" },
  { value: "photos", label: "Photos (no anime enrichment)" },
  { value: "anime", label: "Anime (series and characters in the first pass)" },
];

/** Resolution floor presets, in total pixels (`0` = describe every image). */
const RESOLUTION_FLOORS: { value: number; label: string }[] = [
  { value: 0, label: "Off (describe every image)" },
  { value: 300_000, label: "0.3 MP" },
  { value: 500_000, label: "0.5 MP" },
  { value: 1_000_000, label: "1 MP" },
];

const STARTUP_SCANS: { value: StartupScanSetting; label: string }[] = [
  { value: "ask", label: "Ask every time" },
  { value: "classify_only", label: "Resume classification" },
  { value: "full", label: "Run a full scan" },
  { value: "none", label: "Just open the index" },
];

/** Turn the loaded settings into the payload the command expects. */
export function toSettingsPayload(settings: VisionSettings): SettingsUpdatePayload {
  return {
    model: settings.modelOverride,
    imageMaxSide: settings.imageMaxSide,
    minPixels: settings.minPixels ?? 0,
    libraryKind: settings.libraryKind ?? "auto",
    promptProfile: settings.promptProfile,
    descriptionLanguage: settings.descriptionLanguage,
    imageFilter: settings.imageFilter,
    ocrEngine: settings.ocrEngine,
    startupScan: settings.startupScan,
    dedupeDhashDistance: settings.dedupeDhashDistance,
    chainBacklog: settings.chainBacklog,
    faceDetection: settings.faceDetection,
    faceThreads: settings.faceThreads,
  };
}

/**
 * Every runtime setting in one place.
 *
 * The whole struct is sent in a single `update_settings` call: the backend validates
 * each field with the same parsers `settings.toml` uses and writes nothing when one is
 * invalid, so a typo here cannot leave the file half applied. Model *selection* keeps
 * its own flow (the picker with VRAM calibration); this screen only edits what the
 * pipeline does with the chosen model.
 */
export default function SettingsModal({ onClose, onSaved, onOpenLogs }: Props) {
  const [settings, setSettings] = useState<VisionSettings | null>(null);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  /** Last known state of the description conversion pass (polled while it runs). */
  const [conversion, setConversion] = useState<DescriptionLanguageProgress | null>(null);
  const [converting, setConverting] = useState(false);
  /** Cores and the session split `faceThreads` resolves to, so the hint is concrete. */
  const [threadPlan, setThreadPlan] = useState<FaceThreadPlan | null>(null);
  /** Publishing to the web app: its own state, saved on change. */
  const [publish, setPublish] = useState<PublishConfig>(DEFAULT_PUBLISH);
  const [publishJob, setPublishJob] = useState<PublishProgress | null>(null);
  const [publishing, setPublishing] = useState(false);
  const [lastPublish, setLastPublish] = useState<PublishReport | null>(null);

  useEffect(() => {
    let cancelled = false;
    getVisionSettings()
      .then((loaded) => {
        if (!cancelled) setSettings(loaded);
      })
      .catch((err: unknown) => {
        if (!cancelled) setError(err instanceof Error ? err.message : String(err));
      });
    // A pass may already be running: show its counters instead of a stale "Convert".
    getDescriptionLanguageStatus()
      .then((status) => {
        if (cancelled || !status) return;
        setConversion(status);
        setConverting(status.phase === "converting");
      })
      .catch(() => {
        // The status command is best-effort; a failure here must not block the form.
      });
    getFaceThreadPlan()
      .then((plan) => {
        if (!cancelled) setThreadPlan(plan);
      })
      .catch(() => {
        // Same: the hint degrades to "0 = all cores" without the numbers.
      });
    loadUserConfig()
      .then((config) => {
        if (!cancelled) setPublish(readPublishConfig(config));
      })
      .catch(() => {
        // Without config.json the fields just show the defaults; not worth an error.
      });
    // A publish can finish after this modal was closed, so the last report is read
    // from the backend instead of relying on a promise the screen no longer holds.
    getPublishLastReport()
      .then((report) => {
        if (!cancelled && report) setLastPublish(report);
      })
      .catch(() => {});
    return () => {
      cancelled = true;
    };
  }, []);

  useEffect(() => {
    if (!publishing) return;
    const id = window.setInterval(() => {
      getPublishStatus()
        .then((status) => setPublishJob(status))
        .catch(() => setPublishJob(null));
    }, 500);
    return () => window.clearInterval(id);
  }, [publishing]);

  useEffect(() => {
    if (!converting) return;
    const id = window.setInterval(() => {
      getDescriptionLanguageStatus()
        .then((status) => {
          setConversion(status);
          if (!status || status.phase !== "converting") setConverting(false);
        })
        .catch(() => setConverting(false));
    }, 1000);
    return () => window.clearInterval(id);
  }, [converting]);

  /**
   * Persist what the form shows, then rewrite the descriptions already in the index.
   *
   * Saving first is what makes the button honest: the backend converts into
   * `settings.toml`, not into whatever is selected on screen.
   */
  async function startConversion() {
    if (!settings) return;
    setError(null);
    try {
      const saved = await updateSettings(toSettingsPayload(settings));
      setSettings(saved);
      onSaved?.(saved);
      setConverting(true);
      await convertExistingDescriptions();
    } catch (err: unknown) {
      setConverting(false);
      setError(err instanceof Error ? err.message : String(err));
    }
  }

  async function stopConversion() {
    try {
      await cancelDescriptionConversion();
    } catch (err: unknown) {
      setError(err instanceof Error ? err.message : String(err));
    }
  }

  /** Persist the publish settings immediately; they are deployment config. */
  async function persistPublish(next: PublishConfig) {
    setPublish(next);
    try {
      const config = await loadUserConfig();
      await saveUserConfig({ ...config, publish: next });
    } catch (err: unknown) {
      setError(err instanceof Error ? err.message : String(err));
    }
  }

  async function choosePublishFolder() {
    try {
      const picked = await open({
        directory: true,
        multiple: false,
        title: "Folder the web app will read",
      });
      if (typeof picked === "string") await persistPublish({ ...publish, dir: picked });
    } catch (err: unknown) {
      setError(err instanceof Error ? err.message : String(err));
    }
  }

  async function runPublish() {
    if (!publish.dir.trim()) {
      setError("Choose a publish folder first.");
      return;
    }
    setError(null);
    // The backend reads the folder from config.json, so the screen must have saved
    // what it shows before asking for a publish.
    await persistPublish(publish);
    setPublishing(true);
    setPublishJob(null);
    try {
      const report = await publishLibrary();
      setLastPublish(report);
    } catch (err: unknown) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setPublishing(false);
      setPublishJob(null);
    }
  }

  async function save() {
    if (!settings) return;
    setSaving(true);
    setError(null);
    try {
      const saved = await updateSettings(toSettingsPayload(settings));
      setSettings(saved);
      onSaved?.(saved);
      onClose();
    } catch (err: unknown) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setSaving(false);
    }
  }

  const patch = (next: Partial<VisionSettings>) =>
    setSettings((current) => (current ? { ...current, ...next } : current));

  return (
    <ModalOverlay onBackdropClick={onClose}>
      <div className="modal-base settings-modal" onClick={(e) => e.stopPropagation()}>
        <h3>Settings</h3>
        {!settings && !error && <p className="settings-loading">Loading…</p>}
        {error && <p className="settings-error" role="alert">{error}</p>}
        {settings && (
          <>
            <div className="settings-field">
              <label htmlFor="settings-model">Model</label>
              <input
                id="settings-model"
                type="text"
                value={settings.modelOverride ?? ""}
                placeholder="automatic (chosen from the hardware)"
                onChange={(e) =>
                  patch({ modelOverride: e.target.value.trim() === "" ? null : e.target.value })
                }
              />
              <span className="settings-hint">
                Empty restores the automatic choice. Use the model picker to see what the
                GPU can hold.
              </span>
            </div>

            <div className="settings-field">
              <label htmlFor="settings-max-side">Image max side</label>
              <input
                id="settings-max-side"
                type="number"
                min={0}
                max={4096}
                value={settings.imageMaxSide}
                onChange={(e) => patch({ imageMaxSide: Number(e.target.value) })}
              />
              <span className="settings-hint">
                0 sends the original bytes. Below 768 px the descriptions get worse.
              </span>
            </div>

            <div className="settings-field">
              <label htmlFor="settings-min-pixels">Ignore images below</label>
              <select
                id="settings-min-pixels"
                value={settings.minPixels ?? 0}
                onChange={(e) => patch({ minPixels: Number(e.target.value) })}
              >
                {RESOLUTION_FLOORS.map((option) => (
                  <option key={option.value} value={option.value}>{option.label}</option>
                ))}
              </select>
              <span className="settings-hint">
                Thumbnail and EXIF are still produced, so the file stays in the grid; only
                the vision request is skipped. Lower it later and those rows come back
                without a single new request.
              </span>
            </div>

            <div className="settings-field">
              <label htmlFor="settings-library-kind">Library profile</label>
              <select
                id="settings-library-kind"
                value={settings.libraryKind ?? "auto"}
                onChange={(e) =>
                  patch({ libraryKind: e.target.value as "auto" | "photos" | "anime" })
                }
              >
                {LIBRARY_KINDS.map((option) => (
                  <option key={option.value} value={option.value}>{option.label}</option>
                ))}
              </select>
              <span className="settings-hint">
                Anime asks for series and characters in the first pass, so one image
                request does the work of two; Photos skips that pass entirely.
              </span>
            </div>

            <div className="settings-field">
              <label htmlFor="settings-profile">Prompt profile</label>
              <select
                id="settings-profile"
                value={settings.promptProfile}
                onChange={(e) => patch({ promptProfile: e.target.value as VisionPromptProfile })}
              >
                {PROMPT_PROFILES.map((option) => (
                  <option key={option.value} value={option.value}>{option.label}</option>
                ))}
              </select>
            </div>

            <div className="settings-field">
              <label htmlFor="settings-language">Description language</label>
              <select
                id="settings-language"
                value={settings.descriptionLanguage}
                onChange={(e) =>
                  patch({ descriptionLanguage: e.target.value as DescriptionLanguageSetting })
                }
              >
                {DESCRIPTION_LANGUAGES.map((option) => (
                  <option key={option.value} value={option.value}>{option.label}</option>
                ))}
              </select>
              <span className="settings-hint">
                What new scans write, and what the search box matches on. Identifiers such
                as <code>[Doc: …]</code>, the media types and the OCR text stay English.
              </span>
            </div>

            <div className="settings-field">
              <span className="settings-hint">
                Descriptions already in the index keep their language until they are
                converted. The conversion is text-only — no image is read — so it takes
                minutes instead of a full scan.
              </span>
              {converting ? (
                <div className="settings-conversion">
                  <span>
                    Converting… {conversion?.processed ?? 0} / {conversion?.total ?? 0}
                    {conversion ? ` (${conversion.converted} converted, ${conversion.skipped} skipped, ${conversion.failed} failed)` : ""}
                  </span>
                  <button type="button" onClick={stopConversion}>Cancel</button>
                </div>
              ) : (
                <button type="button" onClick={startConversion} disabled={saving}>
                  Convert existing descriptions
                </button>
              )}
              {!converting && conversion && conversion.phase !== "converting" && (
                <span className="settings-hint" role="status">
                  Last run: {conversion.converted} converted, {conversion.skipped} skipped,{" "}
                  {conversion.failed} failed ({conversion.phase}).
                </span>
              )}
            </div>

            <div className="settings-field">
              <label htmlFor="settings-filter">Resize filter</label>
              <select
                id="settings-filter"
                value={settings.imageFilter}
                onChange={(e) => patch({ imageFilter: e.target.value as ImageFilterSetting })}
              >
                {IMAGE_FILTERS.map((option) => (
                  <option key={option.value} value={option.value}>
                    {option.label} — {option.hint}
                  </option>
                ))}
              </select>
            </div>

            <div className="settings-field">
              <label htmlFor="settings-ocr">OCR engine</label>
              <select
                id="settings-ocr"
                value={settings.ocrEngine}
                onChange={(e) => patch({ ocrEngine: e.target.value as VisionOcrEngine })}
              >
                {OCR_ENGINES.map((option) => (
                  <option key={option.value} value={option.value}>{option.label}</option>
                ))}
              </select>
              <span className="settings-hint">
                Runs only for pictures the model flags as documents.
              </span>
            </div>

            <div className="settings-field">
              <label htmlFor="settings-startup">On startup</label>
              <select
                id="settings-startup"
                value={settings.startupScan}
                onChange={(e) => patch({ startupScan: e.target.value as StartupScanSetting })}
              >
                {STARTUP_SCANS.map((option) => (
                  <option key={option.value} value={option.value}>{option.label}</option>
                ))}
              </select>
            </div>

            <div className="settings-field">
              <label htmlFor="settings-dedupe">Duplicate distance</label>
              <input
                id="settings-dedupe"
                type="number"
                min={0}
                max={16}
                value={settings.dedupeDhashDistance}
                onChange={(e) => patch({ dedupeDhashDistance: Number(e.target.value) })}
              />
              <span className="settings-hint">
                Hamming distance (0–16) under which two photos share one description. 0
                classifies every file.
              </span>
            </div>

            <label className="modal-checkbox settings-checkbox">
              <input
                type="checkbox"
                checked={settings.chainBacklog}
                onChange={(e) => patch({ chainBacklog: e.target.checked })}
              />
              <span>
                A full scan continues into the pipeline: thumbnails and classification in
                one pass, with the CPU work hidden behind the GPU. Off keeps the old
                two-phase scan.
              </span>
            </label>

            <label className="modal-checkbox settings-checkbox">
              <input
                type="checkbox"
                checked={settings.faceDetection}
                onChange={(e) => patch({ faceDetection: e.target.checked })}
              />
              <span>
                Read faces during the scan, on the image the pipeline has already
                decoded. Off by default: it costs CPU inside the pass, and everything it
                skips is still reachable from “Detect Faces”. This is the switch to flip
                when comparing a folder with and without faces.
              </span>
            </label>

            <div className="settings-field">
              <label htmlFor="settings-face-threads">Face detection threads</label>
              <input
                id="settings-face-threads"
                type="number"
                min={0}
                max={32}
                value={settings.faceThreads}
                onChange={(e) => patch({ faceThreads: Number(e.target.value) })}
              />
              <span className="settings-hint">
                0 = every core ({threadPlan?.cores ?? "?"} on this machine). It is a
                ceiling, not a reservation: while a scan is also classifying, faces get
                one thread and the rest stays with the vision model; a pass that only has
                faces left runs with the whole budget. Lower it when the computer is busy
                with something else.
                {threadPlan
                  ? ` Right now: ${threadPlan.budget} thread(s) as ${threadPlan.workers} session(s) of ${threadPlan.intraThreads}.`
                  : ""}
              </span>
            </div>

            <div className="settings-field settings-publish">
              <h4>Publishing to the web app</h4>
              <span className="settings-hint">
                A folder on the shared drive. The desktop writes a consistent snapshot of
                the index and the thumbnails there; the web app only reads it. Keeping the
                index local is deliberate — SQLite must not be written across a share.
              </span>
              <div className="settings-publish-path">
                <input
                  id="settings-publish-dir"
                  type="text"
                  value={publish.dir}
                  placeholder="/mnt/nas/frank-sherlock"
                  aria-label="Publish folder"
                  onChange={(e) => setPublish({ ...publish, dir: e.target.value })}
                  onBlur={() => void persistPublish(publish)}
                />
                <button type="button" onClick={() => void choosePublishFolder()}>
                  Choose…
                </button>
              </div>
              <label className="modal-checkbox settings-checkbox">
                <input
                  type="checkbox"
                  checked={publish.auto}
                  onChange={(e) => void persistPublish({ ...publish, auto: e.target.checked })}
                />
                <span>Publish when a scan finishes.</span>
              </label>
              <label className="modal-checkbox settings-checkbox">
                <input
                  type="checkbox"
                  checked={publish.includeFaceCrops}
                  onChange={(e) =>
                    void persistPublish({ ...publish, includeFaceCrops: e.target.checked })
                  }
                />
                <span>
                  Include face crops — the web app needs them for the people gallery.
                </span>
              </label>
              <label className="settings-inline-field" htmlFor="settings-keep-revisions">
                <span>Snapshots to keep</span>
                <input
                  id="settings-keep-revisions"
                  type="number"
                  min={1}
                  max={10}
                  value={publish.keepRevisions}
                  onChange={(e) =>
                    void persistPublish({ ...publish, keepRevisions: Number(e.target.value) })
                  }
                />
              </label>
              {publishing ? (
                <div className="settings-conversion">
                  <span>
                    Publishing… {publishJob?.phase ?? "starting"}
                    {publishJob && publishJob.total > 0
                      ? ` ${publishJob.done}/${publishJob.total}`
                      : ""}
                  </span>
                  <button type="button" onClick={() => void cancelPublish()}>
                    Cancel
                  </button>
                </div>
              ) : (
                <button
                  type="button"
                  className="settings-logs-btn"
                  onClick={() => void runPublish()}
                  disabled={!publish.dir.trim()}
                >
                  Publish now
                </button>
              )}
              {lastPublish && !publishing && (
                <span className="settings-hint" role="status">
                  Last publish: revision {lastPublish.revision}
                  {lastPublish.cancelled ? " (cancelled)" : ""} —{" "}
                  {lastPublish.thumbnailsCopied} thumbnail(s) copied,{" "}
                  {lastPublish.thumbnailsUnchanged} unchanged,{" "}
                  {lastPublish.revisionsPruned} old snapshot(s) removed, database{" "}
                  {formatBytes(lastPublish.databaseBytes)}.
                </span>
              )}
            </div>

            {onOpenLogs && (
              <div className="settings-field settings-logs">
                <label>Live logs</label>
                <button type="button" className="settings-logs-btn" onClick={onOpenLogs}>
                  Open logs
                </button>
                <span className="settings-hint">
                  The same lines the terminal prints, updated every second — including
                  which photo is being classified right now.
                </span>
              </div>
            )}

            <div className="modal-actions">
              <button type="button" onClick={onClose}>Cancel</button>
              <button type="button" className="primary-btn" onClick={save} disabled={saving}>
                {saving ? "Saving…" : "Save"}
              </button>
            </div>
          </>
        )}
      </div>
    </ModalOverlay>
  );
}
