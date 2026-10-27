import { useState, useRef, useCallback } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import {
  appHealth,
  cancelScan,
  ensureDatabase,
  getRuntimeStatus,
  getScanJob,
  getSetupStatus,
  listActiveScans,
  probeRoots,
  startScan,
  startSetupDownload,
  startVenvProvision,
} from "../api";
import type {
  DbStats,
  RootInfo,
  RuntimeStatus,
  ScanJobStatus,
  ScanMode,
  SetupStatus,
} from "../types";
import { basename, errorMessage } from "../utils";

/** Shallow-compare two scan arrays to avoid unnecessary re-renders. */
function scansChanged(prev: ScanJobStatus[], next: ScanJobStatus[]): boolean {
  if (prev.length !== next.length) return true;
  for (let i = 0; i < prev.length; i++) {
    if (
      prev[i].id !== next[i].id ||
      prev[i].status !== next[i].status ||
      prev[i].processedFiles !== next[i].processedFiles ||
      prev[i].totalFiles !== next[i].totalFiles ||
      prev[i].added !== next[i].added ||
      prev[i].modified !== next[i].modified ||
      prev[i].moved !== next[i].moved ||
      prev[i].phase !== next[i].phase ||
      prev[i].discoveredFiles !== next[i].discoveredFiles
    ) return true;
  }
  return false;
}

const PAGE_SIZE = 80;
const MAX_ITEMS = 400;

type ScanManagerCallbacks = {
  /**
   * Runs before a *classification* scan starts (never for metadata-only
   * refreshes). Resolve `false` to abort. Used for the vision settings modal,
   * which is also where the time projection is shown.
   */
  beforeScan?: (rootPath: string) => Promise<boolean>;
  setSetup: (s: SetupStatus) => void;
  setRuntime: (r: RuntimeStatus) => void;
  setDbStats: (d: DbStats) => void;
  setRoots: (r: RootInfo[]) => void;
  setReadOnly: (ro: boolean) => void;
  setShowResumeModal: (show: boolean) => void;
  setNotice: (msg: string) => void;
  setError: (msg: string) => void;
  runSearch: (offset: number, append: boolean, limitOverride?: number, preserveSelection?: boolean) => Promise<void>;
  itemsLength: () => number;
};

/**
 * What the last poll saw of a running job.
 *
 * `phase`/`total` change when the pass changes *shape* — discovery finishing and
 * writing every row, or the pipeline starting — while `processed` changes every
 * checkpoint. Both mean the grid and the sidebar counters are stale.
 */
type ScanShape = {
  phase: string;
  processed: number;
  total: number;
};

export function useScanManager(cb: ScanManagerCallbacks) {
  const [activeScans, setActiveScans] = useState<ScanJobStatus[]>([]);
  const [trackedJobIds, setTrackedJobIds] = useState<number[]>([]);
  const [completedJobs, setCompletedJobs] = useState<ScanJobStatus[]>([]);
  const lastScanShapeRef = useRef<ScanShape | null>(null);

  const refreshRoots = useCallback(async () => {
    try {
      // probeRoots re-checks reachability and flags offline roots instead of
      // deleting them. It degrades to a plain list on a read-only database.
      const r = await probeRoots();
      cb.setRoots(r);
    } catch {
      // Silently ignore
    }
  }, [cb.setRoots]);

  const pollRuntimeAndScans = useCallback(async () => {
    try {
      const ids = trackedJobIds;
      // A failing runtime probe (nvidia-smi hanging, no GPU driver...) used to reject
      // the whole poll, so scan progress and the grid stopped updating while the
      // backend kept classifying. Those two are informational; the scan is not.
      const [setupStatus, runtimeStatus, scans, ...trackedResults] = await Promise.all([
        getSetupStatus().catch(() => null),
        getRuntimeStatus().catch(() => null),
        listActiveScans(),
        ...ids.map((id) => getScanJob(id).catch(() => null)),
      ]);
      if (setupStatus) cb.setSetup(setupStatus);
      if (runtimeStatus) cb.setRuntime(runtimeStatus);
      setActiveScans((prev) => scansChanged(prev, scans) ? scans : prev);

      if (ids.length === 0) return;

      const trackedJobs = trackedResults.filter((j): j is ScanJobStatus => j !== null);
      const stillTracked: number[] = [];
      const justCompleted: ScanJobStatus[] = [];
      let newest: ScanJobStatus | null = null;
      let anyRunning = false;

      for (const job of trackedJobs) {
        if (job.status === "running" || job.status === "pending" || job.status === "interrupted") {
          stillTracked.push(job.id);
          if (job.status === "running") {
            anyRunning = true;
            if (!newest || job.processedFiles >= newest.processedFiles) newest = job;
          }
        } else if (job.status === "completed") {
          justCompleted.push(job);
        } else if (job.status === "failed") {
          cb.setError(job.errorText || `Scan failed for ${basename(job.rootPath)}`);
        }
      }

      if (anyRunning && newest) {
        const next: ScanShape = {
          phase: newest.phase,
          processed: newest.processedFiles,
          total: newest.totalFiles,
        };
        const prev = lastScanShapeRef.current;
        // The pass changes *shape* when the walk finishes and writes every row, or
        // when the pipeline replaces the thumbnail phase. That is the moment the grid
        // and the sidebar counters become stale, even though no file was processed
        // yet — which is why a 50k folder kept showing the previous 947 files for the
        // whole pipeline.
        const shapeChanged =
          !prev || prev.phase !== next.phase || prev.total !== next.total;
        const progressed = !prev || prev.processed !== next.processed;
        if (shapeChanged || progressed) {
          lastScanShapeRef.current = next;
          const liveLimit = Math.max(PAGE_SIZE, Math.min(cb.itemsLength(), MAX_ITEMS));
          void cb.runSearch(0, false, liveLimit, true);
          void refreshRoots();
          // `ensureDatabase` walks the thumbnail directory, so it is only worth it
          // when the pass changes shape (the row count moved), never per batch.
          if (shapeChanged && (next.total > 0 || next.phase !== "discovering")) {
            ensureDatabase()
              .then((stats) => cb.setDbStats(stats))
              .catch(() => {});
          }
        }
      }

      if (justCompleted.length > 0) {
        lastScanShapeRef.current = null;
        setCompletedJobs((prev) => [...prev, ...justCompleted]);
        const stats = await ensureDatabase();
        cb.setDbStats(stats);
        await refreshRoots();
        await cb.runSearch(0, false);
      }

      setTrackedJobIds(stillTracked);
    } catch (err) {
      cb.setError(errorMessage(err));
    }
  }, [trackedJobIds, cb, refreshRoots]);

  type InitAppResult = {
    roots: RootInfo[];
    scans: ScanJobStatus[];
    setupStatus: SetupStatus;
    readOnly: boolean;
  };

  async function initApp(): Promise<InitAppResult | null> {
    try {
      const [db, setupStatus, runtimeStatus, scans, rootList, health] = await Promise.all([
        ensureDatabase(),
        getSetupStatus(),
        getRuntimeStatus(),
        listActiveScans(),
        probeRoots(),
        appHealth(),
      ]);
      cb.setDbStats(db);
      cb.setSetup(setupStatus);
      cb.setRuntime(runtimeStatus);
      setActiveScans(scans);
      cb.setRoots(rootList);
      cb.setReadOnly(health.readOnly);
      const runningIds = scans.filter((s) => s.status === "running").map((s) => s.id);
      if (runningIds.length > 0) {
        setTrackedJobIds(runningIds);
      }
      const interrupted = scans.filter((s) => s.status === "interrupted");
      if (interrupted.length > 0) {
        cb.setShowResumeModal(true);
      }
      return { roots: rootList, scans, setupStatus, readOnly: health.readOnly };
    } catch (err) {
      cb.setError(errorMessage(err));
      return null;
    }
  }

  /**
   * Starts a scan in `mode`, running the pre-scan prompt (vision settings) for
   * scans that will classify files. Returns the job, or null when the user
   * cancelled or the call failed.
   */
  async function startScanWithMode(rootPath: string, mode: ScanMode, notice: string) {
    if (mode !== "refresh" && cb.beforeScan && !(await cb.beforeScan(rootPath))) {
      return null;
    }
    try {
      setCompletedJobs([]);
      const job = await startScan(rootPath, mode);
      setTrackedJobIds((prev) => [...prev, job.id]);
      // Force the first poll to re-query even when the counters are all zero: the
      // rows may already exist from an earlier interrupted pass.
      lastScanShapeRef.current = null;
      cb.setNotice(notice);
      await refreshRoots();
      await pollRuntimeAndScans();
      return job;
    } catch (err) {
      cb.setError(errorMessage(err));
      return null;
    }
  }

  async function onPickAndScan(setup: SetupStatus | null, readOnly: boolean) {
    if (readOnly) return;
    if (setup && !setup.isReady) {
      cb.setError("Setup is incomplete. Finish Ollama setup before starting scans.");
      return;
    }
    try {
      const selected = await open({ directory: true, multiple: false, title: "Select folder to scan" });
      if (!selected) return;
      const rootPath = selected as string;
      await startScanWithMode(rootPath, "full", `Scan started for ${basename(rootPath)}`);
    } catch (err) {
      cb.setError(errorMessage(err));
    }
  }

  async function onRescanRoot(root: RootInfo, setup: SetupStatus | null, readOnly: boolean) {
    if (readOnly) return;
    if (setup && !setup.isReady) {
      cb.setError("Setup is incomplete. Finish Ollama setup before starting scans.");
      return;
    }
    await startScanWithMode(root.rootPath, "full", `Rescan started for ${root.rootName}`);
  }

  async function onRefreshRoot(root: RootInfo, readOnly: boolean) {
    if (readOnly) return;
    await startScanWithMode(root.rootPath, "refresh", `Refresh started for ${root.rootName}`);
  }

  async function onCancelScan(scan: ScanJobStatus, readOnly: boolean) {
    if (readOnly) return;
    try {
      await cancelScan(scan.id);
      cb.setNotice(`Cancelling scan for ${basename(scan.rootPath)}...`);
      // Poll immediately to pick up the interrupted status faster
      await pollRuntimeAndScans();
    } catch (err) {
      cb.setError(errorMessage(err));
    }
  }

  async function onResumeScan(scan: ScanJobStatus, readOnly: boolean) {
    if (readOnly) return;
    // The job row remembers the phase it died in, so an interrupted backlog pass
    // resumes as a backlog pass instead of turning into a full rescan. A `full`
    // interrupted in the pipeline is in the same situation: discovery already wrote
    // every row, so only the pending work is left (and the walk is the expensive part).
    const mode: ScanMode =
      scan.phase === "backlog" || scan.phase === "processing" ? "backlog" : "full";
    await startScanWithMode(
      scan.rootPath,
      mode,
      `Resuming ${mode === "backlog" ? "backlog" : "scan"} for ${basename(scan.rootPath)}`,
    );
  }

  async function onResumeAllInterrupted() {
    cb.setShowResumeModal(false);
    await Promise.all(
      activeScans
        .filter((s) => s.status === "interrupted")
        .map((scan) => {
          const mode: ScanMode =
            scan.phase === "backlog" || scan.phase === "processing" ? "backlog" : "full";
          return startScanWithMode(
            scan.rootPath,
            mode,
            `Resuming ${mode === "backlog" ? "backlog" : "scan"} for ${basename(scan.rootPath)}`,
          );
        }),
    );
    await pollRuntimeAndScans();
  }

  async function onSetupDownload() {
    try {
      await startSetupDownload();
      await pollRuntimeAndScans();
    } catch (err) {
      cb.setError(errorMessage(err));
    }
  }

  async function onSetupOcr() {
    try {
      await startVenvProvision();
      await pollRuntimeAndScans();
    } catch (err) {
      cb.setError(errorMessage(err));
    }
  }

  async function onRecheckSetup() {
    await pollRuntimeAndScans();
  }

  function addTrackedJobId(id: number) {
    setTrackedJobIds((prev) => [...prev, id]);
  }

  return {
    activeScans,
    trackedJobIds,
    completedJobs,
    setCompletedJobs,
    pollRuntimeAndScans,
    initApp,
    startScanWithMode,
    onPickAndScan,
    onRescanRoot,
    onRefreshRoot,
    onCancelScan,
    onResumeScan,
    onResumeAllInterrupted,
    onSetupDownload,
    onSetupOcr,
    onRecheckSetup,
    refreshRoots,
    addTrackedJobId,
  };
}
