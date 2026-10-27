import { useEffect, useMemo, useRef, useState } from "react";
import { readLogs } from "../../api";
import type { LogEntry } from "../../types";
import ModalOverlay from "./ModalOverlay";
import "./shared-modal.css";
import "./LogsModal.css";

/** One second matches the rest of the app's polling and reads the tail cheaply. */
const POLL_MS = 1000;
/** Enough history to see which files the scan already handled. */
const MAX_LINES = 800;
/** Distance from the bottom under which the viewer counts as "following". */
const FOLLOW_THRESHOLD_PX = 24;

type LevelFilter = "all" | "warn" | "error";

/** Severities from the `log` crate, plus the fallback for unparsed lines. */
const LEVEL_RANK: Record<string, number> = {
  TRACE: 0,
  DEBUG: 1,
  INFO: 2,
  WARN: 3,
  ERROR: 4,
  UNKNOWN: 2,
};

function levelRank(level: string): number {
  return LEVEL_RANK[level.toUpperCase()] ?? 2;
}

function matchesFilter(entry: LogEntry, filter: LevelFilter): boolean {
  const rank = levelRank(entry.level);
  if (filter === "warn") return rank >= LEVEL_RANK.WARN;
  if (filter === "error") return rank >= LEVEL_RANK.ERROR;
  return true;
}

function formatEntry(entry: LogEntry): string {
  const where = entry.target ? `[${entry.target}] ` : "";
  return `${entry.date} ${entry.time} [${entry.level}] ${where}${entry.message}`;
}

type Props = {
  onClose: () => void;
};

/**
 * Live tail of the application log.
 *
 * The backend reads the end of the file `tauri_plugin_log` already writes, so the
 * modal shows the same lines a terminal would — including the `Classifying: …`
 * entry that names the photo being worked on right now. Polling keeps the history
 * available when the modal opens; the viewer only scrolls on its own while the
 * user stays at the bottom, so reading a warning does not get yanked away.
 */
export default function LogsModal({ onClose }: Props) {
  const [entries, setEntries] = useState<LogEntry[]>([]);
  const [logPath, setLogPath] = useState<string | null>(null);
  const [truncated, setTruncated] = useState(false);
  const [levelFilter, setLevelFilter] = useState<LevelFilter>("all");
  const [follow, setFollow] = useState(true);
  const [copied, setCopied] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const listRef = useRef<HTMLDivElement | null>(null);
  const copyTimer = useRef<number | null>(null);

  useEffect(() => {
    let cancelled = false;
    let inFlight = false;

    const tick = async () => {
      if (inFlight) return;
      inFlight = true;
      try {
        const snapshot = await readLogs(MAX_LINES);
        if (cancelled) return;
        setEntries(snapshot.entries);
        setLogPath(snapshot.path);
        setTruncated(snapshot.truncated);
        setError(null);
      } catch (err: unknown) {
        if (!cancelled) setError(err instanceof Error ? err.message : String(err));
      } finally {
        inFlight = false;
      }
    };

    void tick();
    const id = window.setInterval(() => void tick(), POLL_MS);
    return () => {
      cancelled = true;
      window.clearInterval(id);
    };
  }, []);

  const visible = useMemo(
    () => entries.filter((entry) => matchesFilter(entry, levelFilter)),
    [entries, levelFilter],
  );

  // Stay pinned to the newest line unless the user scrolled up to read something.
  useEffect(() => {
    const list = listRef.current;
    if (!list || !follow) return;
    list.scrollTop = list.scrollHeight;
  }, [visible, follow]);

  useEffect(
    () => () => {
      if (copyTimer.current != null) window.clearTimeout(copyTimer.current);
    },
    [],
  );

  function handleScroll() {
    const list = listRef.current;
    if (!list) return;
    const atBottom =
      list.scrollHeight - list.scrollTop - list.clientHeight <= FOLLOW_THRESHOLD_PX;
    setFollow((current) => (current === atBottom ? current : atBottom));
  }

  async function copyVisible() {
    if (!navigator.clipboard || visible.length === 0) return;
    try {
      await navigator.clipboard.writeText(visible.map(formatEntry).join("\n"));
      setCopied(true);
      if (copyTimer.current != null) window.clearTimeout(copyTimer.current);
      copyTimer.current = window.setTimeout(() => setCopied(false), 1500);
    } catch (err: unknown) {
      setError(err instanceof Error ? err.message : String(err));
    }
  }

  return (
    <ModalOverlay onBackdropClick={onClose}>
      <div className="modal-base logs-modal" onClick={(e) => e.stopPropagation()}>
        <div className="logs-header">
          <h3>Live logs</h3>
          <span className="logs-live" title="Updated every second">
            <span className="logs-dot" />
            live
          </span>
        </div>

        <div className="logs-toolbar">
          <label>
            Show
            <select
              aria-label="Log level filter"
              value={levelFilter}
              onChange={(e) => setLevelFilter(e.target.value as LevelFilter)}
            >
              <option value="all">All lines</option>
              <option value="warn">Warnings & errors</option>
              <option value="error">Errors only</option>
            </select>
          </label>
          <label className="logs-follow">
            <input
              type="checkbox"
              checked={follow}
              onChange={(e) => setFollow(e.target.checked)}
            />
            Follow newest
          </label>
          <span className="logs-spacer" />
          <button type="button" onClick={copyVisible} disabled={visible.length === 0}>
            {copied ? "Copied" : "Copy"}
          </button>
          <button type="button" onClick={onClose}>Close</button>
        </div>

        {error && (
          <p className="settings-error" role="alert">{error}</p>
        )}

        <div
          className="logs-list"
          data-testid="logs-list"
          ref={listRef}
          onScroll={handleScroll}
        >
          {visible.length === 0 ? (
            <p className="logs-empty">
              {entries.length === 0
                ? "Nothing logged yet. Start a scan to see the files being processed."
                : "No lines match this filter."}
            </p>
          ) : (
            visible.map((entry, index) => (
              <div
                key={`${entry.date}-${entry.time}-${index}`}
                className={`log-line log-level-${entry.level.toLowerCase()}`}
              >
                <span className="log-time">{entry.date} {entry.time}</span>
                <span className="log-level">{entry.level}</span>
                <span className="log-target" title={entry.target}>{entry.target}</span>
                <span className="log-message">{entry.message}</span>
              </div>
            ))
          )}
        </div>

        <div className="logs-footer">
          <span className="logs-path" title={logPath ?? undefined}>
            {logPath ?? "No log file yet"}
          </span>
          {truncated && <span className="logs-note">older lines omitted</span>}
          <span>{visible.length} of {entries.length} lines</span>
        </div>
      </div>
    </ModalOverlay>
  );
}
