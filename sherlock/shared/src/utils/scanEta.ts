import type { ScanJobStatus } from "../types";

export function formatEta(seconds: number): string {
  if (seconds < 60) return `~${Math.ceil(seconds)}s`;
  const mins = Math.floor(seconds / 60);
  const secs = Math.ceil(seconds % 60);
  if (mins < 60) return `~${mins}m ${secs}s`;
  const hrs = Math.floor(mins / 60);
  const remainMins = mins % 60;
  return `~${hrs}h ${remainMins}m`;
}

// Track when classification actually started (first non-unchanged file processed).
// Module-level so it persists across re-renders but resets on page reload.
let classifyStartTime: number | null = null;
let classifyStartJobId: number | null = null;

export function computeEta(scan: ScanJobStatus): string | null {
  // Only compute ETA for the phases where the GPU is the bottleneck: the sequential
  // classification loop, the pipeline half of a `full`, and a backlog pass (which is
  // the same pipeline). Thumbnailing is fast enough that ETA isn't useful.
  if (
    scan.phase !== "classifying" &&
    scan.phase !== "processing" &&
    scan.phase !== "backlog"
  ) {
    return null;
  }

  const remaining = scan.totalFiles - scan.processedFiles;
  if (remaining <= 0) return null;

  // Need at least 2 processed files for a meaningful rate
  if (scan.processedFiles < 2) return null;

  // Track when classification started for this job
  if (classifyStartJobId !== scan.id) {
    classifyStartTime = null;
    classifyStartJobId = scan.id;
  }
  if (classifyStartTime === null) {
    classifyStartTime = Date.now() / 1000;
  }

  const now = Date.now() / 1000;
  const classifyElapsed = now - classifyStartTime;
  if (classifyElapsed <= 0) return null;

  const avgPerFile = classifyElapsed / scan.processedFiles;
  const etaSeconds = avgPerFile * remaining;
  return formatEta(etaSeconds);
}
