import { useCallback, useEffect, useRef, useState } from "react";
import { cancelScan, getBacklogStatus, listRoots } from "../api";
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
  /**
   * Root of the running pass that stopped answering.
   *
   * A held pass produces no progress at all (nothing is consumed while it waits), so
   * this cannot come from `backlog_status`: it is read from `roots.availability`, which
   * the pass itself updates when it starts and stops waiting.
   */
  const [shareOffline, setShareOffline] = useState<{ rootId: number; rootName: string } | null>(
    null,
  );
  const rootProbeTick = useRef(0);
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

        // While the pass runs, look at the root every few ticks: a share that went away
        // holds the pipeline, and that is the only signal the UI gets for it. One
        // `listRoots` every five polls is cheap next to the 1 s cadence.
        const running = status?.phase === "counting" || status?.phase === "running";
        if (running && status?.rootId) {
          rootProbeTick.current += 1;
          if (rootProbeTick.current % 5 === 1) {
            const roots = await listRoots();
            const root = roots.find((candidate) => candidate.id === status.rootId);
            setShareOffline(
              root?.availability === "offline"
                ? { rootId: root.id, rootName: root.rootName }
                : null,
            );
          }
        } else {
          // Keep the same object when already cleared, so this does not re-render.
          setShareOffline((previous) => (previous ? null : previous));
        }
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

  return { backlog, running, onCancelBacklog, shareOffline };
}
