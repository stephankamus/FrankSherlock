import { useEffect, useState } from "react";
import { convertFileSrc } from "@tauri-apps/api/core";
import { fileName } from "../../utils/format";
import type { SearchItem } from "../../types";

type Props = {
  item: SearchItem;
  index: number;
  isSelected: boolean;
  isFocused: boolean;
  onClick: (index: number, e: React.MouseEvent) => void;
  onDoubleClick: (index: number) => void;
  onContextMenu: (index: number, e: React.MouseEvent) => void;
};

export default function ImageTile({ item, index, isSelected, isFocused, onClick, onDoubleClick, onContextMenu }: Props) {
  const thumb = item.thumbnailPath ? convertFileSrc(item.thumbnailPath) : null;
  // Thumbnails can be missing (stale cache after a root was re-added under a new
  // path, files cleaned up by an earlier scan). Falling back to the original
  // file keeps the grid usable instead of showing a broken image.
  const original = item.absPath ? convertFileSrc(item.absPath) : null;
  const [src, setSrc] = useState<string | null>(thumb);
  useEffect(() => {
    setSrc(thumb);
  }, [thumb]);
  const isVideo = item.mediaType === "video";

  return (
    <article
      className={`tile${isSelected ? " tile-selected" : ""}${isFocused ? " tile-focused" : ""}`}
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
        {item.faceCount != null && item.faceCount > 0 && (
          <span className="tile-face-badge" aria-label={`${item.faceCount} face${item.faceCount > 1 ? "s" : ""}`}>
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
