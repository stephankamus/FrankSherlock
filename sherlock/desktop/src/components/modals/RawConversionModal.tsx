import { useCallback, useEffect, useRef, useState } from "react";
import {
  cancelRawConversion,
  getRawConversionStatus,
  getRawRunningJob,
  getRawSupportStatus,
  provisionRawSupport,
  scanRawCandidates,
  startRawConversion,
} from "../../api";
import type { ConversionOptions, RawJobStatus, RawScanReport, RootInfo } from "../../types";
import { estimateCopyBytes, estimateSeconds } from "../../utils/rawEstimate";
import ModalOverlay from "./ModalOverlay";
import "./shared-modal.css";
import "./RawConversionModal.css";

type Props = {
  root: RootInfo;
  /** Read-only mode: the copies would have to be written into the scanned folder. */
  readOnly?: boolean;
  onCancel: () => void;
  /** Called after a finished job, so the caller can scan and index the new JPEGs. */
  onScanNow: () => void;
};

const MAX_SIDE_CHOICES = [
  { value: 0, label: "Original resolution" },
  { value: 4000, label: "4000 px" },
  { value: 2048, label: "2048 px" },
  { value: 1600, label: "1600 px" },
  { value: 1024, label: "1024 px" },
];

const QUALITY_CHOICES = [80, 88, 92, 95];

const NEF_MODE_CHOICES = [
  { value: "auto", label: "Auto (embedded preview when big enough)" },
  { value: "preview", label: "Embedded preview only (fast)" },
  { value: "demosaic", label: "Full raw development (slow, maximum detail)" },
];

function formatBytes(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes <= 0) return "—";
  const units = ["B", "KB", "MB", "GB", "TB"];
  let value = bytes;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit += 1;
  }
  return `${value >= 10 || unit === 0 ? Math.round(value) : value.toFixed(1)} ${units[unit]}`;
}

/** "8 s", "1 min 20 s", "2 h 05 min" — short enough for a modal line. */
function formatDuration(seconds: number): string {
  if (!Number.isFinite(seconds) || seconds < 0) return "—";
  const total = Math.round(seconds);
  if (total < 60) return `${total} s`;
  if (total < 3600) {
    const minutes = Math.floor(total / 60);
    const rest = total % 60;
    return rest === 0 ? `${minutes} min` : `${minutes} min ${rest} s`;
  }
  const hours = Math.floor(total / 3600);
  const minutes = Math.round((total % 3600) / 60);
  return `${hours} h ${String(minutes).padStart(2, "0")} min`;
}

type JobProgressProps = {
  status: RawJobStatus | null;
  running: boolean;
  finished: boolean;
  done: number;
  total: number;
  percent: number;
  elapsedSeconds: number;
  remainingSeconds: number;
};

/**
 * Progress and final report.
 *
 * Extracted because it has to render in two situations: right after a scan (the modal owns
 * the numbers) and after reattaching to a job that kept running while the modal was closed
 * — in that case there is no scan at all, and the job's own status is the only truth.
 */
function JobProgress({
  status,
  running,
  finished,
  done,
  total,
  percent,
  elapsedSeconds,
  remainingSeconds,
}: JobProgressProps) {
  return (
    <>
      {running && (
        <div className="raw-progress" data-testid="raw-progress">
          <progress value={done} max={Math.max(total, 1)} />
          {status?.phase === "scanning" || total === 0 ? (
            // The walk has no total to show until it ends; saying so beats a frozen
            // `0 / ?` for minutes.
            <p className="raw-progress-line" data-testid="raw-progress-line">
              <strong>Scanning the folder…</strong>
              {elapsedSeconds >= 1 && (
                <span className="raw-progress-elapsed"> · {formatDuration(elapsedSeconds)}</span>
              )}
            </p>
          ) : (
            <p className="raw-progress-line" data-testid="raw-progress-line">
              <strong>
                {done} / {total} ({percent}%)
              </strong>
              {total > done && <span> · ~{formatDuration(remainingSeconds)} remaining</span>}
              {elapsedSeconds >= 1 && (
                <span className="raw-progress-elapsed">
                  {" "}
                  · {formatDuration(elapsedSeconds)} elapsed
                </span>
              )}
            </p>
          )}
          {status?.current && <p className="raw-progress-current">{status.current}</p>}
        </div>
      )}

      {finished && status && (
        <div className="raw-result" data-testid="raw-result">
          {status.report.failed > 0 && status.report.converted === 0 ? (
            <p>The conversion did not run:</p>
          ) : (
            <p>
              {status.report.converted} converted, {status.report.skipped} skipped,{" "}
              {status.report.failed} failed
              {status.report.bytesOut > 0
                ? ` — ${formatBytes(status.report.bytesOut)} written`
                : ""}
              {status.report.cancelled ? " (cancelled)" : ""}
            </p>
          )}
          {status.report.failures.length > 0 && (
            <ul className="raw-failures">
              {status.report.failures.map((failure) => (
                <li key={failure}>{failure}</li>
              ))}
            </ul>
          )}
        </div>
      )}
    </>
  );
}

/**
 * Explains why HEIC/NEF files are missing from the index and converts them.
 *
 * This is the only place in the app that writes inside a scanned folder: the copies go to
 * `FrankSherlock-JPG/` next to the originals, they are ordinary JPEGs, and the next scan
 * indexes them like any other photo. Nothing is written without the checkbox, and a
 * read-only folder has no alternative — the modal says so instead of pretending.
 *
 * The folder is walked **once**, when the modal opens. Changing an option only
 * recomputes the estimates locally: a walk of a large library per keystroke is exactly
 * what made this modal feel frozen.
 */
export default function RawConversionModal({ root, readOnly, onCancel, onScanNow }: Props) {
  const [options, setOptions] = useState<ConversionOptions>({
    maxSide: 0,
    quality: 90,
    nefMode: "auto",
  });
  const [report, setReport] = useState<RawScanReport | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [accepted, setAccepted] = useState(false);
  const [installing, setInstalling] = useState(false);
  const [supportReady, setSupportReady] = useState(false);
  const [jobId, setJobId] = useState<number | null>(null);
  const [status, setStatus] = useState<RawJobStatus | null>(null);
  const [finished, setFinished] = useState(false);
  /** A start request is in flight: the button must not fire twice. */
  const [starting, setStarting] = useState(false);
  /** Checked folders, by relative path; `""` is the root itself. */
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [rescanning, setRescanning] = useState(false);

  const mounted = useRef(true);
  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
    };
  }, []);

  const refresh = useCallback(async (force = false) => {
    setError(null);
    try {
      const next = await scanRawCandidates(root.id, force);
      if (!mounted.current) return;
      setReport(next);
      setSupportReady(next.conversionReady);
      setSelected((previous) => {
        const available = new Set(next.dirs.map((dir) => dir.relPath));
        // Keep what the user already chose; on the first load (or when a rescan drops
        // every checked folder) select everything, which is the useful default.
        const kept = new Set([...previous].filter((dir) => available.has(dir)));
        return kept.size > 0 ? kept : available;
      });
    } catch (e) {
      if (mounted.current) setError(String(e));
    }
  }, [root.id]);

  const rescan = async () => {
    setRescanning(true);
    await refresh(true);
    if (mounted.current) setRescanning(false);
  };

  // Opening the modal either reattaches to a job that kept running after it was closed,
  // or walks the folder once. Reattaching skips the walk: the conversion already has the
  // list, and a second traversal would only delay the first sign of life.
  useEffect(() => {
    let active = true;
    (async () => {
      const runningJob = await getRawRunningJob(root.id).catch(() => null);
      if (!active || !mounted.current) return;
      if (runningJob != null) {
        setJobId(runningJob);
        setAccepted(true);
        return;
      }
      await refresh();
    })();
    return () => {
      active = false;
    };
  }, [root.id, refresh]);

  // Poll the job, like every other long task in this app (face detection, recluster,
  // thumbnail repair): one mutex-guarded status struct read on an interval.
  useEffect(() => {
    if (jobId == null) return;
    let active = true;
    const timer = setInterval(async () => {
      try {
        const next = await getRawConversionStatus(jobId);
        if (!active || !mounted.current) return;
        setStatus(next);
        if (!next.running) {
          setFinished(true);
          clearInterval(timer);
        }
      } catch (e) {
        if (active && mounted.current) setError(String(e));
        clearInterval(timer);
      }
    }, 700);
    return () => {
      active = false;
      clearInterval(timer);
    };
  }, [jobId]);

  const start = async () => {
    // The backend also refuses a second job for the same root, but the guard here
    // closes the window between the click and the IPC reply.
    if (starting || jobId != null) return;
    setStarting(true);
    setError(null);
    try {
      const id = await startRawConversion(root.id, options, [...selected]);
      if (!mounted.current) return;
      setJobId(id);
      setFinished(false);
      setStatus(null);
    } catch (e) {
      if (mounted.current) setError(String(e));
    } finally {
      if (mounted.current) setStarting(false);
    }
  };

  const toggleDir = (relPath: string, checked: boolean) => {
    setSelected((previous) => {
      const next = new Set(previous);
      if (checked) next.add(relPath);
      else next.delete(relPath);
      return next;
    });
  };

  const toggleAllDirs = (checked: boolean) => {
    setSelected(checked ? new Set(report?.dirs.map((dir) => dir.relPath) ?? []) : new Set());
  };

  const cancelJob = async () => {
    if (jobId == null) return;
    try {
      await cancelRawConversion(jobId);
    } catch (e) {
      setError(String(e));
    }
  };

  const install = async () => {
    setInstalling(true);
    setError(null);
    try {
      await provisionRawSupport();
    } catch (e) {
      setError(String(e));
      setInstalling(false);
    }
  };

  // The venv install runs in the background; ask the backend until the interpreter can
  // really import both decoders (a venv directory exists long before `pip` finishes).
  useEffect(() => {
    if (!installing) return;
    const timer = setInterval(async () => {
      try {
        const support = await getRawSupportStatus();
        if (!mounted.current) return;
        if (support.ready) {
          setSupportReady(true);
          setInstalling(false);
          clearInterval(timer);
        }
      } catch {
        // Keep polling: `pip` is still running.
      }
    }, 2000);
    return () => clearInterval(timer);
  }, [installing]);

  // Everything the modal shows is derived from the **checked** folders, so unchecking one
  // immediately lowers the counts, the estimates and the Convert button.
  const activeDirs = report ? report.dirs.filter((dir) => selected.has(dir.relPath)) : [];
  const allDirsChecked = !!report && report.dirs.length > 0 && activeDirs.length === report.dirs.length;
  const someDirsChecked = activeDirs.length > 0 && !allDirsChecked;
  const selectedCounts = activeDirs.reduce(
    (acc, dir) => ({
      heic: acc.heic + dir.heic,
      nef: acc.nef + dir.nef,
      bytes: acc.bytes + dir.sourceBytes,
      converted: acc.converted + dir.alreadyConverted,
    }),
    { heic: 0, nef: 0, bytes: 0, converted: 0 },
  );
  // Per-directory rows do not carry a pending count; `alreadyConverted` is the closest
  // cheap answer. When every folder is checked the exact number from the report wins.
  const selection: RawScanReport | null = report
    ? {
        ...report,
        dirs: activeDirs,
        heic: selectedCounts.heic,
        nef: selectedCounts.nef,
        totalFiles: selectedCounts.heic + selectedCounts.nef,
        sourceBytes: selectedCounts.bytes,
        pending: allDirsChecked
          ? report.pending
          : Math.max(
              0,
              selectedCounts.heic + selectedCounts.nef - selectedCounts.converted,
            ),
      }
    : null;

  const pending = selection?.pending ?? 0;
  const running = status?.running ?? false;
  const canStart =
    !readOnly &&
    !running &&
    !starting &&
    // `jobId` stays set for the life of the modal: between the click and the first poll
    // (up to 700 ms) `running` is still false, and the button must not come back.
    jobId === null &&
    supportReady &&
    pending > 0 &&
    selected.size > 0 &&
    accepted;

  const estimatedSeconds = selection ? estimateSeconds(selection, options) : 0;
  const estimatedBytes = selection ? estimateCopyBytes(selection, options) : 0;

  // Progress and time: the job reports done/total, so the remaining time is a simple
  // extrapolation of the observed rate (per-file cost varies a lot between formats,
  // which is why the pre-start estimate and this one are both labelled as estimates).
  const done = status?.done ?? 0;
  const total = status?.total ?? 0;
  const percent = total > 0 ? Math.min(100, Math.round((done / total) * 100)) : 0;
  const elapsedSeconds =
    running && status?.startedAtMs ? Math.max(0, (Date.now() - status.startedAtMs) / 1000) : 0;
  const remainingSeconds =
    running && done > 0 && total > done
      ? (elapsedSeconds / done) * (total - done)
      : running
        ? estimatedSeconds
        : 0;

  return (
    <ModalOverlay onBackdropClick={onCancel}>
      <div className="modal-base raw-modal" onClick={(e) => e.stopPropagation()}>
        <h3>Convert HEIC/NEF photos</h3>

        {!report && !error && <p className="raw-loading">Scanning the folder…</p>}

        {report && (
          <>
            <p>
              HEIC and NEF files cannot be read by the vision pipeline, a browser or the
              indexer, so they never appear in this folder. This converts them into JPEG
              copies that the normal scan picks up. The originals are never modified.
            </p>

            <dl className="raw-summary" data-testid="raw-summary">
              <div>
                <dt>HEIC</dt>
                <dd>{selection?.heic ?? 0}</dd>
              </div>
              <div>
                <dt>NEF</dt>
                <dd>{selection?.nef ?? 0}</dd>
              </div>
              <div>
                <dt>Already converted</dt>
                <dd>{Math.max(0, (selection?.totalFiles ?? 0) - pending)}</dd>
              </div>
              <div>
                <dt>To convert</dt>
                <dd data-testid="raw-pending">{pending}</dd>
              </div>
              <div>
                <dt>Originals</dt>
                <dd>{formatBytes(selection?.sourceBytes ?? 0)}</dd>
              </div>
              <div>
                <dt>Estimated copies</dt>
                <dd data-testid="raw-estimate">
                  {pending > 0 ? formatBytes(estimatedBytes) : "—"}
                </dd>
              </div>
              <div>
                <dt>Estimated time</dt>
                <dd data-testid="raw-eta">
                  {pending > 0 ? `~${formatDuration(estimatedSeconds)}` : "—"}
                </dd>
              </div>
            </dl>

            {report.dirs.length > 0 && (
              <div className="raw-dir-list">
                <div className="raw-dir-header">
                  <label className="raw-dir-title">
                    <input
                      type="checkbox"
                      aria-label="Select all folders"
                      checked={allDirsChecked}
                      disabled={running}
                      ref={(element) => {
                        if (element) element.indeterminate = someDirsChecked;
                      }}
                      onChange={(e) => toggleAllDirs(e.target.checked)}
                    />
                    <span>
                      Folders ({activeDirs.length}/{report.dirs.length})
                    </span>
                  </label>
                  <button
                    type="button"
                    className="raw-rescan"
                    onClick={rescan}
                    disabled={running || rescanning}
                  >
                    {rescanning ? "Rescanning…" : "Rescan"}
                  </button>
                </div>
                <ul className="raw-dirs" data-testid="raw-dirs">
                  {report.dirs.map((dir) => (
                    <li key={dir.relPath || "."} className={selected.has(dir.relPath) ? "" : "raw-dir-off"}>
                      <label className="raw-dir-row">
                        <input
                          type="checkbox"
                          aria-label={`Convert ${dir.relPath || "the root folder"}`}
                          checked={selected.has(dir.relPath)}
                          disabled={running}
                          onChange={(e) => toggleDir(dir.relPath, e.target.checked)}
                        />
                        <code>{dir.relPath || "(root)"}</code>
                      </label>
                      <span>
                        {dir.heic + dir.nef} file{dir.heic + dir.nef === 1 ? "" : "s"}
                        {dir.alreadyConverted > 0 ? ` · ${dir.alreadyConverted} done` : ""}
                      </span>
                    </li>
                  ))}
                </ul>
              </div>
            )}

            <div className="raw-options">
              <label>
                <span>Resolution</span>
                <select
                  aria-label="Copy resolution"
                  value={options.maxSide}
                  disabled={running}
                  onChange={(e) =>
                    setOptions((prev) => ({ ...prev, maxSide: Number(e.target.value) }))
                  }
                >
                  {MAX_SIDE_CHOICES.map((choice) => (
                    <option key={choice.value} value={choice.value}>
                      {choice.label}
                    </option>
                  ))}
                </select>
              </label>
              <label>
                <span>JPEG quality</span>
                <select
                  aria-label="JPEG quality"
                  value={options.quality}
                  disabled={running}
                  onChange={(e) =>
                    setOptions((prev) => ({ ...prev, quality: Number(e.target.value) }))
                  }
                >
                  {QUALITY_CHOICES.map((quality) => (
                    <option key={quality} value={quality}>
                      {quality}
                    </option>
                  ))}
                </select>
              </label>
              <label>
                <span>NEF development</span>
                <select
                  aria-label="NEF development"
                  value={options.nefMode}
                  disabled={running}
                  onChange={(e) => setOptions((prev) => ({ ...prev, nefMode: e.target.value }))}
                >
                  {NEF_MODE_CHOICES.map((choice) => (
                    <option key={choice.value} value={choice.value}>
                      {choice.label}
                    </option>
                  ))}
                </select>
              </label>
            </div>

            {!supportReady && (
              <div className="raw-support" data-testid="raw-support">
                <p>
                  The conversion support is a separate Python environment with
                  <code> pillow-heif</code> and <code>rawpy</code>. {report.conversionMessage}
                </p>
                <button type="button" onClick={install} disabled={installing}>
                  {installing ? "Installing…" : "Install conversion support"}
                </button>
              </div>
            )}

            {readOnly && (
              <p className="raw-warning">
                This folder was added in read-only mode, so nothing can be written next to
                the originals.
              </p>
            )}

            <label className="modal-checkbox">
              <input
                type="checkbox"
                checked={accepted}
                disabled={running || readOnly}
                onChange={(e) => setAccepted(e.target.checked)}
              />
              <span>
                Create JPEG copies in a <code>FrankSherlock-JPG</code> folder inside each
                directory above. This is the one place the app writes into a scanned
                folder. If the folder is synced (Dropbox, Syncthing, iCloud), the copies
                will be synced too. A read-only directory is reported per-file and skipped.
              </span>
            </label>

            {error && <p className="raw-error">{error}</p>}

            <JobProgress
              status={status}
              running={running}
              finished={finished}
              done={done}
              total={total}
              percent={percent}
              elapsedSeconds={elapsedSeconds}
              remainingSeconds={remainingSeconds}
            />
          </>
        )}

        {/* A reattached job has no scan of its own, so progress lives outside the guard. */}
        {!report && (
          <JobProgress
            status={status}
            running={running}
            finished={finished}
            done={done}
            total={total}
            percent={percent}
            elapsedSeconds={elapsedSeconds}
            remainingSeconds={remainingSeconds}
          />
        )}

        {error && !report && <p className="raw-error">{error}</p>}

        <div className="modal-actions">
          <button
            type="button"
            onClick={() => {
              // Closing must not leave a job running that the user can no longer see.
              if (running) void cancelJob();
              onCancel();
            }}
          >
            {running ? "Cancel and close" : "Close"}
          </button>
          {running && (
            <button type="button" onClick={cancelJob}>
              Cancel
            </button>
          )}
          {finished && (status?.report.converted ?? 0) > 0 && (
            <button type="button" onClick={onScanNow}>
              Scan now
            </button>
          )}
          {!finished && (
            <button type="button" className="primary-btn" disabled={!canStart} onClick={start}>
              {starting ? "Starting…" : `Convert ${pending > 0 ? pending : ""}`.trim()}
            </button>
          )}
        </div>
      </div>
    </ModalOverlay>
  );
}
