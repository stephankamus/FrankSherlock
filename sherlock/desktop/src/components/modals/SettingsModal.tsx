import { useEffect, useState } from "react";
import { getVisionSettings, updateSettings } from "../../api";
import type {
  ImageFilterSetting,
  SettingsUpdatePayload,
  StartupScanSetting,
  VisionOcrEngine,
  VisionPromptProfile,
  VisionSettings,
} from "../../types";
import ModalOverlay from "./ModalOverlay";
import "./shared-modal.css";
import "./SettingsModal.css";

type Props = {
  onClose: () => void;
  /** Called with what the backend saved, so the caller can refresh its state. */
  onSaved?: (settings: VisionSettings) => void;
};

const PROMPT_PROFILES: { value: VisionPromptProfile; label: string }[] = [
  { value: "concise", label: "Concise (fewer output tokens)" },
  { value: "detailed", label: "Detailed (adds anime candidates)" },
];

const IMAGE_FILTERS: { value: ImageFilterSetting; label: string; hint: string }[] = [
  { value: "auto", label: "Auto", hint: "box for JPEG/HEIC, Lanczos3 for PNG/BMP/TIFF" },
  { value: "box", label: "Box", hint: "fastest; measured −33 % on the downscale" },
  { value: "lanczos", label: "Lanczos3", hint: "sharpest text, three times slower" },
];

const OCR_ENGINES: { value: VisionOcrEngine; label: string }[] = [
  { value: "llm", label: "LLM (multimodal model)" },
  { value: "surya", label: "Surya (isolated venv)" },
  { value: "off", label: "Off" },
];

const STARTUP_SCANS: { value: StartupScanSetting; label: string }[] = [
  { value: "ask", label: "Ask every time" },
  { value: "classify_only", label: "Resume classification" },
  { value: "full", label: "Run a full scan" },
  { value: "none", label: "Just open the index" },
];

/** Turn the loaded settings into the payload the command expects. */
function toPayload(settings: VisionSettings): SettingsUpdatePayload {
  return {
    model: settings.modelOverride,
    imageMaxSide: settings.imageMaxSide,
    promptProfile: settings.promptProfile,
    imageFilter: settings.imageFilter,
    ocrEngine: settings.ocrEngine,
    startupScan: settings.startupScan,
    dedupeDhashDistance: settings.dedupeDhashDistance,
    chainBacklog: settings.chainBacklog,
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
export default function SettingsModal({ onClose, onSaved }: Props) {
  const [settings, setSettings] = useState<VisionSettings | null>(null);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    getVisionSettings()
      .then((loaded) => {
        if (!cancelled) setSettings(loaded);
      })
      .catch((err: unknown) => {
        if (!cancelled) setError(err instanceof Error ? err.message : String(err));
      });
    return () => {
      cancelled = true;
    };
  }, []);

  async function save() {
    if (!settings) return;
    setSaving(true);
    setError(null);
    try {
      const saved = await updateSettings(toPayload(settings));
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
