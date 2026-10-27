import { useState, useEffect, useRef, useCallback } from "react";
import type { BacklogProgress, FaceDetectProgress, RootInfo, ScanJobStatus } from "../../types";
import { computeEta } from "../../utils/scanEta";

type RootCardProps = {
  root: RootInfo;
  isSelected: boolean;
  scan: ScanJobStatus | undefined;
  readOnly: boolean;
  faceProgress?: FaceDetectProgress;
  /** Live backlog counters for this root, when the backlog pass is running. */
  backlogProgress?: BacklogProgress;
  onSelect: () => void;
  onDelete: () => void;
  onRescan: () => void;
  onRefresh: () => void;
  onCopyPath: () => void;
  onRelink?: () => void;
  onDetectFaces?: () => void;
  /** Read faces inside the next scan (mirrors the Settings field). */
  faceDetection?: boolean;
  onToggleFaceDetection?: () => void;
  /** Clear the classification of this folder so it can be sent to the model again. */
  onReclassify?: () => void;
  onCancelScan?: () => void;
  onResumeScan?: () => void;
  onCancelFaceDetect?: () => void;
};

export default function RootCard({ root, isSelected, scan, readOnly, faceProgress, backlogProgress, onSelect, onDelete, onRescan, onRefresh, onCopyPath, onRelink, onDetectFaces, faceDetection, onToggleFaceDetection, onReclassify, onCancelScan, onResumeScan, onCancelFaceDetect }: RootCardProps) {
  const [showMenu, setShowMenu] = useState(false);
  const [menuPos, setMenuPos] = useState({ x: 0, y: 0 });
  const menuRef = useRef<HTMLDivElement>(null);

  const closeMenu = useCallback(() => setShowMenu(false), []);

  const progress = scan?.totalFiles
    ? Math.min(100, (scan.processedFiles / Math.max(1, scan.totalFiles)) * 100)
    : 0;

  function handleContextMenu(e: React.MouseEvent) {
    if (readOnly) return;
    e.preventDefault();
    e.stopPropagation();
    setMenuPos({ x: e.clientX, y: e.clientY });
    setShowMenu(true);
  }

  // Viewport clamping
  useEffect(() => {
    if (!showMenu) return;
    const el = menuRef.current;
    if (!el) return;
    const rect = el.getBoundingClientRect();
    if (rect.right > window.innerWidth) {
      el.style.left = `${window.innerWidth - rect.width - 4}px`;
    }
    if (rect.bottom > window.innerHeight) {
      el.style.top = `${window.innerHeight - rect.height - 4}px`;
    }
  }, [showMenu, menuPos]);

  // Click-away and Escape dismiss
  useEffect(() => {
    if (!showMenu) return;
    function handleMouseDown(e: MouseEvent) {
      if (menuRef.current && !menuRef.current.contains(e.target as Node)) {
        closeMenu();
      }
    }
    function handleKeyDown(e: KeyboardEvent) {
      if (e.key === "Escape") closeMenu();
    }
    document.addEventListener("mousedown", handleMouseDown);
    document.addEventListener("keydown", handleKeyDown);
    return () => {
      document.removeEventListener("mousedown", handleMouseDown);
      document.removeEventListener("keydown", handleKeyDown);
    };
  }, [showMenu, closeMenu]);

  const eta = scan?.status === "running" ? computeEta(scan) : null;

  return (
    <div
      className={`root-card${isSelected ? " selected" : ""}`}
      onClick={onSelect}
      onContextMenu={handleContextMenu}
      title={root.rootPath}
      role="button"
      tabIndex={0}
      onKeyDown={(e) => {
        if (e.key === "Enter" || e.key === " ") {
          e.preventDefault();
          onSelect();
        }
      }}
    >
      <div className="root-card-header">
        <span className="root-card-icon">
          <svg width="16" height="16" viewBox="0 0 16 16" fill="none" xmlns="http://www.w3.org/2000/svg">
            <path d="M1.5 3C1.5 2.17 2.17 1.5 3 1.5h3.59a1.5 1.5 0 011.06.44L8.71 3H13a1.5 1.5 0 011.5 1.5v8A1.5 1.5 0 0113 14H3a1.5 1.5 0 01-1.5-1.5V3z" fill="#5AC8FA"/>
            <path d="M1.5 5.5h13v7a1.5 1.5 0 01-1.5 1.5H3a1.5 1.5 0 01-1.5-1.5v-7z" fill="#34AADC"/>
            <path d="M1.5 5.5h13v1H1.5z" fill="rgba(0,0,0,0.08)"/>
          </svg>
        </span>
        <span className="root-card-name" title={root.rootPath}>{root.rootName}</span>
        {!readOnly && (
          <button
            type="button"
            className="root-card-delete"
            onClick={(e) => { e.stopPropagation(); onDelete(); }}
            title="Remove folder"
            aria-label={`Remove ${root.rootName}`}
          >&times;</button>
        )}
      </div>
      <div className="root-card-meta">
        <span>{root.fileCount.toLocaleString()} files</span>
        {root.availability === "offline" && (
          <span
            className="root-card-offline"
            title={root.lastError ?? "Folder is not reachable right now"}
          >
            Offline
          </span>
        )}
      </div>
      {root.availability === "offline" && (
        <div className="root-card-scan root-card-scan-offline">
          <span>Folder not reachable. {root.fileCount.toLocaleString()} files stay indexed.</span>
          {!readOnly && onRelink && (
            <button
              type="button"
              className="root-card-scan-btn"
              onClick={(e) => { e.stopPropagation(); onRelink(); }}
            >Locate…</button>
          )}
        </div>
      )}
      {scan?.status === "running" && scan.phase === "discovering" && (
        <div className="root-card-scan">
          <div className="root-card-discovery-bar" />
          <span>Discovering files... ({scan.discoveredFiles.toLocaleString()} found)</span>
          {!readOnly && onCancelScan && (
            <button type="button" className="root-card-scan-btn" onClick={(e) => { e.stopPropagation(); onCancelScan(); }}>Pause</button>
          )}
        </div>
      )}
      {scan?.status === "running" && scan.phase === "thumbnailing" && (
        <div className="root-card-scan">
          <progress value={progress} max={100} />
          <span>Thumbnailing {scan.processedFiles}/{scan.totalFiles}</span>
          <div className="root-card-scan-stats">
            +{scan.added} new, {scan.modified} mod, {scan.moved} moved
          </div>
          {!readOnly && onCancelScan && (
            <button type="button" className="root-card-scan-btn" onClick={(e) => { e.stopPropagation(); onCancelScan(); }}>Pause</button>
          )}
        </div>
      )}
      {scan?.status === "running" &&
        (scan.phase === "classifying" ||
          scan.phase === "processing" ||
          scan.phase === "backlog") && (
        <div className="root-card-scan">
          <progress value={progress} max={100} />
          <span>
            {scan.phase === "classifying"
              ? `Classifying ${scan.processedFiles.toLocaleString()}/${scan.totalFiles.toLocaleString()}`
              : `${backlogProgress?.mode === "retry_failed" ? "Retrying" : "Processing"} ${scan.processedFiles.toLocaleString()}/${scan.totalFiles.toLocaleString()}`}
          </span>
          {eta && <span className="root-card-eta">{eta} remaining</span>}
          {backlogProgress && (
            <>
              <div className="root-card-scan-stats">
                Thumbnails: {backlogProgress.thumbnailed.toLocaleString()} /{" "}
                {backlogProgress.thumbsTotal.toLocaleString()}
              </div>
              <div className="root-card-scan-stats">
                Classified: {backlogProgress.classified.toLocaleString()} /{" "}
                {backlogProgress.classifyTotal.toLocaleString()}
              </div>
              {backlogProgress.failed > 0 && (
                <div className="root-card-scan-stats">
                  Failed: {backlogProgress.failed.toLocaleString()}
                </div>
              )}
            </>
          )}
          {!readOnly && onCancelScan && (
            <button type="button" className="root-card-scan-btn" onClick={(e) => { e.stopPropagation(); onCancelScan(); }}>Pause</button>
          )}
        </div>
      )}
      {/* A record without a phase-specific panel (e.g. the root card did not get the
          job in `activeScans`) still shows the pass counters instead of nothing. */}
      {backlogProgress &&
        !(
          scan?.status === "running" &&
          (scan.phase === "classifying" ||
            scan.phase === "processing" ||
            scan.phase === "backlog")
        ) && (
        <div className="root-card-scan">
          <progress
            value={
              backlogProgress.total > 0
                ? Math.min(100, (backlogProgress.processed / backlogProgress.total) * 100)
                : 0
            }
            max={100}
          />
          <span>
            Backlog {backlogProgress.processed.toLocaleString()}/
            {backlogProgress.total.toLocaleString()}
          </span>
          <div className="root-card-scan-stats">
            {backlogProgress.thumbnailed.toLocaleString()} thumbnail(s),{" "}
            {backlogProgress.classified.toLocaleString()} classified
            {(backlogProgress.faces ?? 0) > 0
              ? `, ${backlogProgress.faces.toLocaleString()} face(s)`
              : ""}
            {backlogProgress.failed > 0 ? `, ${backlogProgress.failed} failed` : ""}
          </div>
          {!readOnly && onCancelScan && (
            <button type="button" className="root-card-scan-btn" onClick={(e) => { e.stopPropagation(); onCancelScan(); }}>Stop</button>
          )}
        </div>
      )}
      {scan?.status === "interrupted" && (
        <div className="root-card-scan root-card-scan-interrupted">
          <span>Scan interrupted</span>
          {!readOnly && onResumeScan && (
            <button type="button" className="root-card-scan-btn" onClick={(e) => { e.stopPropagation(); onResumeScan(); }}>Resume</button>
          )}
        </div>
      )}
      {faceProgress && faceProgress.phase === "downloading" && (
        <div className="root-card-scan">
          <div className="root-card-discovery-bar" />
          <span>Downloading face models...</span>
        </div>
      )}
      {faceProgress && faceProgress.phase === "loading" && (
        <div className="root-card-scan">
          <div className="root-card-discovery-bar" />
          <span>Loading face models...</span>
        </div>
      )}
      {faceProgress && faceProgress.phase === "detecting" && (
        <div className="root-card-scan">
          <progress value={faceProgress.processed} max={faceProgress.total} />
          <span>Detecting faces {faceProgress.processed}/{faceProgress.total} ({faceProgress.facesFound} found)</span>
          {!readOnly && onCancelFaceDetect && (
            <button type="button" className="root-card-scan-btn" onClick={(e) => { e.stopPropagation(); onCancelFaceDetect(); }}>Cancel</button>
          )}
        </div>
      )}
      {showMenu && (
        <div
          ref={menuRef}
          className="root-card-context-menu"
          style={{ left: menuPos.x, top: menuPos.y }}
          role="menu"
        >
          <button role="menuitem" onClick={(e) => { e.stopPropagation(); onCopyPath(); setShowMenu(false); }}>Copy Path</button>
          {root.availability === "offline" && onRelink && (
            <button role="menuitem" onClick={(e) => { e.stopPropagation(); onRelink(); setShowMenu(false); }}>Locate Folder…</button>
          )}
          <button role="menuitem" onClick={(e) => { e.stopPropagation(); onRefresh(); setShowMenu(false); }}>Refresh Metadata</button>
          <button role="menuitem" onClick={(e) => { e.stopPropagation(); onRescan(); setShowMenu(false); }}>Rescan</button>
          {onToggleFaceDetection && (
            <button
              role="menuitemcheckbox"
              aria-checked={Boolean(faceDetection)}
              title="Read faces while scanning, on the image the pipeline already decoded. Off by default; Detect Faces reads whatever is left."
              onClick={(e) => { e.stopPropagation(); onToggleFaceDetection(); setShowMenu(false); }}
            >
              {faceDetection ? "✓ " : "  "}Read faces during scans
            </button>
          )}
          {onDetectFaces && (
            <button role="menuitem" onClick={(e) => { e.stopPropagation(); onDetectFaces(); setShowMenu(false); }}>Detect Faces</button>
          )}
          {onReclassify && (
            <button role="menuitem" onClick={(e) => { e.stopPropagation(); onReclassify(); setShowMenu(false); }}>Reclassify</button>
          )}
          <button role="menuitem" className="danger" onClick={(e) => { e.stopPropagation(); onDelete(); setShowMenu(false); }}>Remove</button>
        </div>
      )}
    </div>
  );
}
