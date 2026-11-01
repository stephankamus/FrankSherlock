import { useState } from "react";

import ModalOverlay from "./ModalOverlay";
import type { RefineOptions } from "../types";
import "./modal.css";
import "./refine-faces-modal.css";

type Props = {
  /** Names shown in the header; the people whose faces will be regrouped. */
  peopleNames: string[];
  busy: boolean;
  onConfirm: (options: RefineOptions) => void;
  onClose: () => void;
};

type Preset = { id: string; label: string; hint: string; threshold: number };

const PRESETS: Preset[] = [
  { id: "balanced", label: "Balanced", hint: "Splits clear mismatches", threshold: 0.45 },
  { id: "strict", label: "Strict", hint: "Recommended for people lumped together", threshold: 0.55 },
  { id: "very-strict", label: "Very strict", hint: "Many small, very pure groups", threshold: 0.65 },
];

export default function RefineFacesModal({ peopleNames, busy, onConfirm, onClose }: Props) {
  const [presetId, setPresetId] = useState("strict");
  const [threshold, setThreshold] = useState(0.55);
  const [minConfidence, setMinConfidence] = useState(0.65);
  const [minFacePx, setMinFacePx] = useState(40);
  const [minGroupSize, setMinGroupSize] = useState(2);
  const [matchExisting, setMatchExisting] = useState(true);

  function choosePreset(p: Preset) {
    setPresetId(p.id);
    setThreshold(p.threshold);
  }

  const title =
    peopleNames.length === 1 ? `Re-analyze "${peopleNames[0]}"` : `Re-analyze ${peopleNames.length} people`;

  return (
    <ModalOverlay onBackdropClick={busy ? undefined : onClose}>
      <div
        className="modal-base refine-faces-modal"
        onClick={(e) => e.stopPropagation()}
        data-testid="refine-faces-modal"
      >
        <h3>{title}</h3>
        <p>
          Every face of the selection is compared again with a stricter similarity, so different
          people that were grouped together are separated. Each new group joins an existing person
          when it matches, otherwise it becomes a new person.
        </p>

        <div className="refine-presets" role="radiogroup" aria-label="Strictness">
          {PRESETS.map((p) => (
            <button
              key={p.id}
              type="button"
              role="radio"
              aria-checked={presetId === p.id}
              className={`refine-preset${presetId === p.id ? " active" : ""}`}
              onClick={() => choosePreset(p)}
              disabled={busy}
            >
              <strong>{p.label}</strong>
              <span>{p.hint}</span>
            </button>
          ))}
        </div>

        <details className="refine-advanced">
          <summary>Advanced</summary>
          <label>
            Similarity threshold ({threshold.toFixed(2)})
            <input
              type="range"
              min={0.3}
              max={0.8}
              step={0.01}
              value={threshold}
              disabled={busy}
              onChange={(e) => {
                setPresetId("custom");
                setThreshold(Number(e.target.value));
              }}
            />
          </label>
          <label>
            Minimum detection confidence ({minConfidence.toFixed(2)})
            <input
              type="range"
              min={0.5}
              max={0.95}
              step={0.01}
              value={minConfidence}
              disabled={busy}
              onChange={(e) => setMinConfidence(Number(e.target.value))}
            />
          </label>
          <label>
            Minimum face size (px)
            <input
              type="number"
              min={16}
              max={400}
              value={minFacePx}
              disabled={busy}
              onChange={(e) => setMinFacePx(Math.max(16, Number(e.target.value) || 16))}
            />
          </label>
          <label>
            Minimum faces per new person
            <input
              type="number"
              min={1}
              max={50}
              value={minGroupSize}
              disabled={busy}
              onChange={(e) => setMinGroupSize(Math.max(1, Number(e.target.value) || 1))}
            />
          </label>
        </details>

        <label className="refine-check">
          <input
            type="checkbox"
            checked={matchExisting}
            disabled={busy}
            onChange={(e) => setMatchExisting(e.target.checked)}
          />
          Match against people already recognized
        </label>

        <div className="modal-actions">
          <button type="button" onClick={onClose} disabled={busy}>
            Cancel
          </button>
          <button
            type="button"
            disabled={busy}
            onClick={() => onConfirm({ threshold, minConfidence, minFacePx, minGroupSize, matchExisting })}
          >
            {busy ? "Re-analyzing..." : "Re-analyze"}
          </button>
        </div>
      </div>
    </ModalOverlay>
  );
}
