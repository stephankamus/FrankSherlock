import { useEffect, useState } from "react";

import type { SearchItem } from "../types";
import { useLibrary } from "../useLibrary";
import { fileName } from "../utils/format";
import "./library.css";

type Props = {
  item: SearchItem;
  index: number;
  isSelected: boolean;
  isFocused: boolean;
  onClick: (index: number, e: React.MouseEvent) => void;
  onDoubleClick: (index: number) => void;
  onContextMenu: (index: number, e: React.MouseEvent) => void;
  /** Hides or unhides this photo. Absent on the read-only web client. */
  onToggleHidden?: (index: number) => void;
};

/**
 * One grid tile.
 *
 * The only platform-specific part is where the pixels come from: the desktop client
 * returns `asset://` URLs for the cached thumbnail and the original file, the HTTP
 * client returns `/api/thumb/{id}` and a resized `/api/media/{id}?w=`. The web never
 * receives a stored path, and this component never touches the filesystem.
 */
export default function ImageTile({
  item,
  index,
  isSelected,
  isFocused,
  onClick,
  onDoubleClick,
  onContextMenu,
  onToggleHidden,
}: Props) {
  const library = useLibrary();
  const thumb = library.thumbUrl(item);
  // Thumbnails can be missing (stale cache after a root was re-added under a new
  // path, files cleaned up by an earlier scan). Falling back to the original keeps
  // the grid usable instead of showing a broken image; on the web that fallback is
  // the resized derivative, not a 12 MP JPEG. A video cannot render in an <img>,
  // so it goes straight to the placeholder.
  const original = item.mediaType === "video" ? null : library.previewUrl(item, 1600) || null;
  const [src, setSrc] = useState<string | null>(thumb);
  useEffect(() => {
    setSrc(thumb);
  }, [thumb]);
  const isVideo = item.mediaType === "video";
  // Only ever set while the "show hidden" filter is on: the query drops hidden rows
  // otherwise, so a badge here means the user asked to see them.
  const isHidden = item.hiddenAt != null;
  const canHide = library.capabilities.write && onToggleHidden != null;

  return (
    <article
      className={`tile${isSelected ? " tile-selected" : ""}${isFocused ? " tile-focused" : ""}${isHidden ? " tile-hidden" : ""}`}
      role="listitem"
      onClick={(e) => onClick(index, e)}
      onDoubleClick={() => onDoubleClick(index)}
      onContextMenu={(e) => onContextMenu(index, e)}
    >
      <div className="tile-thumb">
        {src ? (
          <img
            src={src}
            alt={item.relPath}
            loading="lazy"
            onError={() => {
              if (original && src !== original) {
                setSrc(original);
              } else {
                setSrc(null);
              }
            }}
          />
        ) : (
          <div className="tile-thumb-placeholder">
            <span className="badge">{item.mediaType}</span>
          </div>
        )}
        {isVideo && (
          <span className="tile-video-badge" aria-label="Video">&#9654;</span>
        )}
        {canHide && (
          // The click must not reach the tile: selecting the photo is a different
          // gesture, and on a multi-selection it would clear it.
          <button
            type="button"
            className={`tile-hide-btn${isHidden ? " tile-hide-btn-active" : ""}`}
            title={isHidden ? "Unhide this photo" : "Hide this photo"}
            aria-label={isHidden ? "Unhide this photo" : "Hide this photo"}
            onClick={(e) => {
              e.stopPropagation();
              onToggleHidden?.(index);
            }}
            onDoubleClick={(e) => e.stopPropagation()}
          >
            <svg width="14" height="14" viewBox="0 0 16 16" fill="currentColor" aria-hidden="true">
              <path d="M8 3C4.5 3 1.7 5.4 1 8c.7 2.6 3.5 5 7 5s6.3-2.4 7-5c-.7-2.6-3.5-5-7-5zm0 8.2A3.2 3.2 0 118 4.8a3.2 3.2 0 010 6.4z"/>
              <circle cx="8" cy="8" r="1.6"/>
            </svg>
          </button>
        )}
        {isHidden && (
          <span className="tile-hidden-badge" title="Hidden — not shown in the web app">
            hidden
          </span>
        )}
        {item.sourceFormat && (
          // A JPEG copy made from a HEIC/NEF file: the grid shows a normal photo, and
          // this chip is what tells the user (and the duplicate view) where it came
          // from. `title` carries the original path for the full story.
          <span
            className="tile-source-badge"
            title={`Converted from ${item.sourceRelPath ?? item.sourceFormat.toUpperCase()}`}
          >
            {item.sourceFormat.toUpperCase()}
          </span>
        )}
        {item.faceCount != null && item.faceCount > 0 && (
          // Informative only: clicking the tile already opens this photo. The title
          // is what explains the blue number, which the hover overlay used to hide.
          <span
            className="tile-face-badge"
            title={`${item.faceCount} face${item.faceCount > 1 ? "s" : ""} detected in this photo`}
            aria-label={`${item.faceCount} face${item.faceCount > 1 ? "s" : ""} detected`}
          >
            {item.faceCount}
          </span>
        )}
      </div>
      <div className="tile-filename">
        <span>{fileName(item.relPath)}</span>
      </div>
      <div className="tile-hover-overlay">
        <h3>{fileName(item.relPath)}</h3>
        <p>{item.description || "No description yet"}</p>
        <div className="tile-meta">
          <span className="badge">{item.mediaType}</span>
          <span>{item.confidence.toFixed(2)}</span>
        </div>
      </div>
    </article>
  );
}
