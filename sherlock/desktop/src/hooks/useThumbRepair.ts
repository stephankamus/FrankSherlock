import { useCallback, useEffect, useRef, useState } from "react";
import { cancelThumbRepair, getThumbRepairStatus, repairThumbnails } from "../api";
import type { ThumbRepairProgress } from "../types";
import { errorMessage } from "../utils";

type ThumbRepairCallbacks = {
  pollMs: number;
  onNotice: (msg: string) => void;
  onError: (msg: string) => void;
  /** Called once when a run finishes, to refresh derived state (e.g. db stats). */
  onFinished?: () => void;
};

/**
 * Drives the `Repair Thumbnails` tool.
 *
 * Same shape as `useFaceDetection`: the backend keeps a single in-memory
 * progress record, the hook polls it and surfaces a failure exactly once.
 */
export function useThumbRepair({ pollMs, onNotice, onError, onFinished }: ThumbRepairCallbacks) {
  const [thumbRepair, setThumbRepair] = useState<ThumbRepairProgress | null>(null);
  const failureReported = useRef(false);
  const finishedReported = useRef(true);
  // Kept in a ref so a new inline callback on every render does not reset the
  // polling interval (which would starve it during frequent re-renders).
  const finishedRef = useRef(onFinished);
  finishedRef.current = onFinished;

  useEffect(() => {
    const id = setInterval(async () => {
      try {
        const status = await getThumbRepairStatus();
        if (status?.phase === "failed") {
          if (!failureReported.current) {
            failureReported.current = true;
            onError(status.error || "Thumbnail repair failed");
          }
          setThumbRepair(null);
          return;
        }
        failureReported.current = false;
        setThumbRepair(status);
        if (
          (status?.phase === "done" || status?.phase === "cancelled") &&
          !finishedReported.current
        ) {
          finishedReported.current = true;
          onNotice(
            status.phase === "done"
              ? `Repaired ${status.repaired} thumbnail(s)`
              : `Thumbnail repair cancelled after ${status.repaired} file(s)`,
          );
          finishedRef.current?.();
        }
      } catch {
        /* ignore polling errors */
      }
    }, pollMs);
    return () => clearInterval(id);
  }, [pollMs, onError, onNotice]);

  const onRepairThumbnails = useCallback(
    async (rootId?: number | null) => {
      try {
        await repairThumbnails(rootId ?? null);
        finishedReported.current = false;
        setThumbRepair({
          rootId: rootId ?? null,
          total: 0,
          processed: 0,
          repaired: 0,
          failed: 0,
          startedAt: Math.floor(Date.now() / 1000),
          phase: "scanning",
          error: null,
        });
      } catch (err) {
        onError(errorMessage(err));
      }
    },
    [onError],
  );

  const onCancelThumbRepair = useCallback(async () => {
    try {
      await cancelThumbRepair();
    } catch (err) {
      onError(errorMessage(err));
    }
  }, [onError]);

  const repairing =
    thumbRepair != null &&
    (thumbRepair.phase === "scanning" || thumbRepair.phase === "repairing");

  return { thumbRepair, repairing, onRepairThumbnails, onCancelThumbRepair };
}
