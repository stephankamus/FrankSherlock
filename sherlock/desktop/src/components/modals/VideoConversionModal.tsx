import { useEffect, useRef, useState } from "react";
import {
  cancelVideoConversion,
  getVideoConversionStatus,
  startVideoConversion,
  videoWebScan,
} from "../../api";
import type { RootInfo, VideoJobStatus, VideoWebReport } from "../../types";
import ModalOverlay from "./ModalOverlay";
import "./shared-modal.css";
import "./RawConversionModal.css";

type Props = {
  root: RootInfo;
  readOnly?: boolean;
  onCancel: () => void;
};

const HEIGHT_CHOICES = [
  { value: 0, label: "Original size" },
  { value: 1080, label: "1080p" },
  { value: 720, label: "720p" },
  { value: 480, label: "480p" },
];
const CRF_CHOICES = [
  { value: 20, label: "High quality (bigger files)" },
  { value: 23, label: "Balanced" },
  { value: 28, label: "Small files" },
];

// Rough x264 "veryfast" throughput relative to playback time; a remux is near instant.
const TRANSCODE_SPEED = 2.5;

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

function formatDuration(seconds: number): string {
  if (!Number.isFinite(seconds) || seconds < 0) return "—";
  const total = Math.round(seconds);
  if (total < 60) return `${total} s`;
  if (total < 3600) return `${Math.floor(total / 60)} min ${total % 60} s`;
  return `${Math.floor(total / 3600)} h ${String(Math.round((total % 3600) / 60)).padStart(2, "0")} min`;
}

export default function VideoConversionModal({ root, readOnly, onCancel }: Props) {
  const [report, setReport] = useState<VideoWebReport | null>(null);
  const [status, setStatus] = useState<VideoJobStatus | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [maxHeight, setMaxHeight] = useState(1080);
  const [crf, setCrf] = useState(23);
  const [accepted, setAccepted] = useState(false);
  const [starting, setStarting] = useState(false);
  const [attached, setAttached] = useState(false);
  const mounted = useRef(true);

  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
    };
  }, []);

  // Reattach to a job that is already running for this root; otherwise scan.
  useEffect(() => {
    let active = true;
    (async () => {
      try {
        const current = await getVideoConversionStatus();
        if (!active) return;
        if (current.running && current.rootId === root.id) {
          setStatus(current);
          setAttached(true);
          return;
        }
        const next = await videoWebScan(root.id);
        if (active) setReport(next);
      } catch (e) {
        if (active) setError(String(e));
      }
    })();
    return () => {
      active = false;
    };
  }, [root.id]);

  useEffect(() => {
    if (!attached) return;
    let active = true;
    const timer = setInterval(async () => {
      try {
        const next = await getVideoConversionStatus();
        if (!active || !mounted.current) return;
        setStatus(next);
        if (!next.running) clearInterval(timer);
      } catch (e) {
        if (active && mounted.current) setError(String(e));
        clearInterval(timer);
      }
    }, 700);
    return () => {
      active = false;
      clearInterval(timer);
    };
  }, [attached]);

  const start = async () => {
    if (starting || attached) return;
    setStarting(true);
    setError(null);
    try {
      await startVideoConversion(root.id, { maxHeight: maxHeight || null, crf });
      if (!mounted.current) return;
      setStatus(null);
      setAttached(true);
    } catch (e) {
      if (mounted.current) setError(String(e));
    } finally {
      if (mounted.current) setStarting(false);
    }
  };

  const cancelJob = async () => {
    try {
      await cancelVideoConversion();
    } catch (e) {
      setError(String(e));
    }
  };

  const running = status?.running ?? false;
  const finished = attached && status !== null && !status.running;
  const done = status?.done ?? 0;
  const total = status?.total ?? 0;
  const elapsed =
    running && status?.startedAtMs ? Math.max(0, (Date.now() - status.startedAtMs) / 1000) : 0;
  const remaining =
    running && status && status.doneSecs > 0 && status.totalSecs > status.doneSecs
      ? (elapsed / status.doneSecs) * (status.totalSecs - status.doneSecs)
      : 0;
  const estimate = report
    ? report.transcodeSecs / TRANSCODE_SPEED + report.remux * 5
    : 0;
  const canStart =
    !readOnly && !attached && !starting && !!report && report.ffmpegReady && report.total > 0 && accepted;

  return (
    <ModalOverlay onBackdropClick={onCancel}>
      <div className="modal-base raw-modal" onClick={(e) => e.stopPropagation()}>
        <h3>Convert videos for the web</h3>

        {!report && !attached && !error && <p className="raw-loading">Scanning videos…</p>}

        {report && !attached && (
          <>
            <p>
              Some videos cannot be played by a browser (for example HEVC, MPEG-4, AVI, MOV or
              MPG). This writes a browser-friendly MP4 copy next to the original, inside a{" "}
              <code>FrankSherlock-MP4</code> folder. Originals are never modified and the copies
              are not added to the library.
            </p>
            <dl className="raw-summary" data-testid="video-summary">
              <div>
                <dt>To convert</dt>
                <dd>{report.total}</dd>
              </div>
              <div>
                <dt>Re-encode (slow)</dt>
                <dd>{report.transcode}</dd>
              </div>
              <div>
                <dt>Rewrap (fast)</dt>
                <dd>{report.remux}</dd>
              </div>
              <div>
                <dt>Original size</dt>
                <dd>{formatBytes(report.totalBytes)}</dd>
              </div>
              <div>
                <dt>Estimated time</dt>
                <dd>{report.total > 0 ? `~${formatDuration(estimate)}` : "—"}</dd>
              </div>
            </dl>
            {report.groups.length > 0 && (
              <ul>
                {report.groups.map((g) => (
                  <li key={g.label}>
                    {g.label}: {g.count}
                  </li>
                ))}
              </ul>
            )}
            {!report.ffmpegReady && <p className="raw-error">{report.message}</p>}
            {report.total === 0 && report.ffmpegReady && (
              <p>Every video in this root already plays in a browser.</p>
            )}
            {report.total > 0 && (
              <>
                <label>
                  Max height (re-encoded videos){" "}
                  <select value={maxHeight} onChange={(e) => setMaxHeight(Number(e.target.value))}>
                    {HEIGHT_CHOICES.map((c) => (
                      <option key={c.value} value={c.value}>
                        {c.label}
                      </option>
                    ))}
                  </select>
                </label>
                <label>
                  Quality{" "}
                  <select value={crf} onChange={(e) => setCrf(Number(e.target.value))}>
                    {CRF_CHOICES.map((c) => (
                      <option key={c.value} value={c.value}>
                        {c.label}
                      </option>
                    ))}
                  </select>
                </label>
                <label>
                  <input
                    type="checkbox"
                    checked={accepted}
                    onChange={(e) => setAccepted(e.target.checked)}
                  />{" "}
                  I allow writing copies into the folders of this root
                </label>
              </>
            )}
          </>
        )}

        {attached && (
          <div className="raw-progress" data-testid="video-progress">
            <progress value={done} max={Math.max(total, 1)} />
            <p className="raw-progress-line">
              <strong>
                {done} / {total}
              </strong>
              {running && remaining > 0 && <span> · ~{formatDuration(remaining)} remaining</span>}
              {elapsed >= 1 && <span> · {formatDuration(elapsed)} elapsed</span>}
            </p>
            {status?.current && <p className="raw-progress-current">{status.current}</p>}
            {finished && status && (
              <p>
                {status.cancelled ? "Cancelled. " : "Done. "}
                {status.converted} converted, {status.failed} failed
                {status.outBytes > 0 ? ` (${formatBytes(status.outBytes)} written)` : ""}.
              </p>
            )}
            {status?.fatal && <p className="raw-error">{status.fatal}</p>}
            {status && status.errors.length > 0 && (
              <ul>
                {status.errors.map((e, i) => (
                  <li key={i}>{e}</li>
                ))}
              </ul>
            )}
          </div>
        )}

        {error && <p className="raw-error">{error}</p>}

        <div className="modal-actions">
          {running ? (
            <button type="button" onClick={cancelJob}>
              Cancel conversion
            </button>
          ) : (
            <button type="button" onClick={onCancel}>
              Close
            </button>
          )}
          {!attached && (
            <button type="button" className="primary-btn" disabled={!canStart} onClick={start}>
              Convert
            </button>
          )}
        </div>
      </div>
    </ModalOverlay>
  );
}
