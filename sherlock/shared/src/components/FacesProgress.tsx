import { useRef } from "react";
import type { ReclusterProgress } from "../types";

const STEP_LABELS: Record<string, string> = {
  crops: "Regenerating face crops",
  assigning: "Assigning faces to people",
  comparing: "Comparing people to find duplicates",
  merging: "Merging similar people",
  pruning: "Cleaning up misplaced faces",
};

/** Wait this long in a step before trusting its speed enough to estimate the rest. */
const MIN_SECONDS_FOR_ESTIMATE = 3;

export function formatDuration(totalSeconds: number): string {
  const seconds = Math.max(0, Math.round(totalSeconds));
  if (seconds < 60) return `${seconds}s`;
  const minutes = Math.floor(seconds / 60);
  if (minutes < 60) return `${minutes}m ${String(seconds % 60).padStart(2, "0")}s`;
  return `${Math.floor(minutes / 60)}h ${String(minutes % 60).padStart(2, "0")}m`;
}

/** Remaining time of the current step, or null while there is not enough data. */
export function estimateRemainingSeconds(
  processed: number,
  total: number,
  stepElapsedSeconds: number,
): number | null {
  if (total <= 0 || processed <= 0 || processed > total) return null;
  if (stepElapsedSeconds < MIN_SECONDS_FOR_ESTIMATE) return null;
  return (stepElapsedSeconds * (total - processed)) / processed;
}

function stepKey(status: ReclusterProgress): string {
  return status.phase === "crops" ? "crops" : status.stage || status.phase;
}

type Props = {
  status: ReclusterProgress;
  /** What the run is doing as a whole, e.g. "Re-clustering" or "Grouping new faces". */
  title: string;
};

/**
 * Progress of a face grouping run: the step it is in, how far that step is, and a time
 * estimate for it. The estimate comes from the speed seen so far in the current step, so
 * it only appears after a few seconds and restarts when the run moves to the next step.
 */
export default function FacesProgress({ status, title }: Props) {
  const runStart = useRef(Date.now());
  const step = useRef({ key: stepKey(status), startedAt: Date.now() });

  const key = stepKey(status);
  if (step.current.key !== key) step.current = { key, startedAt: Date.now() };

  const now = Date.now();
  const stepElapsed = (now - step.current.startedAt) / 1000;
  const runElapsed = (now - runStart.current) / 1000;
  const { processed, total } = status;
  const label = STEP_LABELS[key] ?? "Working";

  if (status.phase === "done") {
    return <div className="faces-progress">Done! Reloading...</div>;
  }

  let detail: string;
  let percent: number | null = null;
  if (total > 0) {
    percent = Math.min(100, Math.round((processed / total) * 100));
    const remaining = estimateRemainingSeconds(processed, total, stepElapsed);
    detail =
      `${processed.toLocaleString("en-US")} of ${total.toLocaleString("en-US")} (${percent}%)` +
      (remaining !== null ? ` · about ${formatDuration(remaining)} left` : " · estimating...");
  } else if (key === "merging") {
    detail = `${processed.toLocaleString("en-US")} merged so far`;
  } else {
    detail = "starting...";
  }

  return (
    <div className="faces-progress" role="status">
      <div className="faces-progress-title">
        <strong>{title}</strong> — {label}
      </div>
      <div className="faces-progress-detail">
        {detail} · elapsed {formatDuration(runElapsed)}
      </div>
      <div className="faces-progress-bar" aria-hidden="true">
        <div
          className={`faces-progress-fill${percent === null ? " indeterminate" : ""}`}
          style={percent === null ? undefined : { width: `${percent}%` }}
        />
      </div>
    </div>
  );
}
