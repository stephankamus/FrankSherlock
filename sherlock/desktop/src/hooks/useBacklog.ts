import { useCallback, useEffect, useRef, useState } from "react";
import { cancelScan, getBacklogStatus } from "../api";
import type { BacklogProgress } from "../types";
import { errorMessage } from "../utils";

type BacklogCallbacks = {
  pollMs: number;
  onNotice: (msg: string) => void;
  onError: (msg: string) => void;
  /** Called once when a run finishes, to refresh derived state (e.g. db stats). */
  onFinished?: () => void;
};

/**
 * Drives the producer/consumer pipeline: the "Process Backlog" tool, the retry
 * pass, and the pipeline half of a `full` scan.
 *
 * Same shape as `useThumbRepair`: the backend keeps one in-memory progress record
 * (the scan job row only carries a single counter), this hook polls it and
 * surfaces each stage separately, because a row can need a thumbnail, a
 * classification, or both.
 *
 * Cancelling goes through the normal `cancel_scan`: a backlog pass *is* a scan
 * job, so it shares the cancellation flag.
 */
export function useBacklog({ pollMs, onNotice, onError, onFinished }: BacklogCallbacks) {
  const [backlog, setBacklog] = useState<BacklogProgress | null>(null);
  const failureReported = useRef(false);
  const finishedReported = useRef(true);
  // Kept in a ref so a new inline callback on every render does not reset the
  // polling interval (which would starve it during frequent re-renders).
  const finishedRef = useRef(onFinished);
  finishedRef.current = onFinished;

  useEffect(() => {
    const id = setInterval(async () => {
      try {
        const status = await getBacklogStatus();
        if (status?.phase === "failed") {
          if (!failureReported.current) {
            failureReported.current = true;
            onError(status.error || "Backlog failed");
          }
          setBacklog(null);
          return;
        }
        failureReported.current = false;
        if (status?.phase === "counting" || status?.phase === "running") {
          finishedReported.current = false;
        }
        setBacklog(status);
        if (
          (status?.phase === "done" || status?.phase === "cancelled") &&
          !finishedReported.current
        ) {
          finishedReported.current = true;
          // A `full` scan runs the same pipeline, so the notice has to say "Scan":
          // calling it "Backlog" made the toast contradict the root card.
          const label = status.mode === "full" ? "Scan" : "Backlog";
          onNotice(
            status.phase === "done"
              ? `${label} done: ${status.classified} classified, ${status.thumbnailed} thumbnail(s)`
              : `${label} stopped after ${status.processed} file(s)`,
          );
          finishedRef.current?.();
        }
      } catch {
        /* ignore polling errors */
      }
    }, pollMs);
    return () => clearInterval(id);
  }, [pollMs, onError, onNotice]);

  const onCancelBacklog = useCallback(async () => {
    if (!backlog) return;
    try {
      await cancelScan(backlog.jobId);
    } catch (err) {
      onError(errorMessage(err));
    }
  }, [backlog, onError]);

  const running =
    backlog != null && (backlog.phase === "counting" || backlog.phase === "running");

  return { backlog, running, onCancelBacklog };
}
