import { useEffect, useState, type ReactNode } from "react";

import type { SearchItem } from "../types";
import { useLibrary } from "../useLibrary";
import { fileName, formatBytes, isPdfPath, isVideoPath } from "../utils/format";
import ModalOverlay from "./ModalOverlay";
import "./modal.css";
import "./preview.css";

type Props = {
  previewItems: SearchItem[];
  selectedCount: number;
  singlePreviewIndex: number | null;
  totalItems: number;
  onClose: () => void;
  onNavigate: (index: number) => void;
  /**
   * Renders one PDF pane. The desktop passes its `PdfViewer` (react-pdf + saved
   * passwords); the web omits it and gets the same-origin `<iframe>` plus a download
   * link, because unlocking a password is a desktop-only feature in v1.
   */
  renderPdf?: (item: SearchItem) => ReactNode;
};

type LayoutMode =
  | "single-image"
  | "single-pdf"
  | "single-video"
  | "image-collage"
  | "dual-pdf"
  | "pdf-plus-image";

function detectLayout(items: SearchItem[]): {
  mode: LayoutMode;
  pdfs: SearchItem[];
  images: SearchItem[];
} {
  const pdfs = items.filter((item) => isPdfPath(item.relPath)).slice(0, 2);
  const images = items.filter((item) => !isPdfPath(item.relPath) && !isVideoPath(item.relPath));

  if (items.length === 1) {
    if (isVideoPath(items[0].relPath)) {
      return { mode: "single-video", pdfs, images };
    }
    return isPdfPath(items[0].relPath)
      ? { mode: "single-pdf", pdfs, images }
      : { mode: "single-image", pdfs, images };
  }

  if (pdfs.length >= 2) {
    return { mode: "dual-pdf", pdfs: pdfs.slice(0, 2), images: [] };
  }

  if (pdfs.length === 1 && images.length >= 1) {
    return { mode: "pdf-plus-image", pdfs, images: images.slice(0, 1) };
  }

  return { mode: "image-collage", pdfs: [], images };
}

/** Resolve the playback URL through the client (stream server on the desktop). */
function useVideoStreamUrl(item: SearchItem | undefined): string | null {
  const library = useLibrary();
  const [url, setUrl] = useState<string | null>(null);

  useEffect(() => {
    if (!item) {
      setUrl(null);
      return;
    }
    let cancelled = false;
    library
      .videoUrl(item)
      .then((resolved) => {
        if (!cancelled) setUrl(resolved || null);
      })
      .catch(() => {
        if (!cancelled) setUrl(null);
      });
    return () => {
      cancelled = true;
    };
  }, [item, library]);

  return url;
}

export default function PreviewModal({
  previewItems,
  selectedCount,
  singlePreviewIndex,
  totalItems,
  onClose,
  onNavigate,
  renderPdf,
}: Props) {
  const library = useLibrary();
  // A caller may hand over the whole result page plus the index the user clicked (the
  // web grid does). When a single item is being previewed, the index — not the array
  // length — decides the layout; otherwise every click opened a collage of the page.
  const singleItem =
    singlePreviewIndex != null
      ? previewItems[
          Math.min(Math.max(singlePreviewIndex, 0), Math.max(previewItems.length - 1, 0))
        ]
      : undefined;
  const layoutItems = singleItem ? [singleItem] : previewItems;
  const { mode, pdfs, images } = detectLayout(layoutItems);
  const videoItem = mode === "single-video" ? layoutItems[0] : undefined;
  const videoUrl = useVideoStreamUrl(videoItem);
  const [videoError, setVideoError] = useState(false);

  // Reset error when navigating to a different item
  useEffect(() => { setVideoError(false); }, [videoItem?.id]);

  // Keyboard: Space closes preview, arrows navigate (single-item only)
  useEffect(() => {
    function handleKeyDown(e: KeyboardEvent) {
      const tag = (e.target as HTMLElement).tagName;
      if (tag === "INPUT" || tag === "SELECT" || tag === "TEXTAREA" || tag === "VIDEO") return;

      if (e.key === " ") {
        e.preventDefault();
        e.stopPropagation();
        onClose();
        return;
      }

      if (singlePreviewIndex !== null) {
        if (e.key === "ArrowLeft" && singlePreviewIndex > 0) {
          e.preventDefault();
          e.stopPropagation();
          onNavigate(singlePreviewIndex - 1);
        } else if (e.key === "ArrowRight" && singlePreviewIndex < totalItems - 1) {
          e.preventDefault();
          e.stopPropagation();
          onNavigate(singlePreviewIndex + 1);
        }
      }
    }
    document.addEventListener("keydown", handleKeyDown, true);
    return () => document.removeEventListener("keydown", handleKeyDown, true);
  }, [onClose, onNavigate, singlePreviewIndex, totalItems]);

  function pdfPane(item: SearchItem) {
    if (renderPdf) return renderPdf(item);
    const url = library.pdfUrl(item);
    return (
      <>
        <iframe className="preview-pdf-frame" src={url} title={fileName(item.relPath)} />
        <a className="preview-pdf-download" href={url} download={fileName(item.relPath)}>
          Download PDF
        </a>
      </>
    );
  }

  return (
    <ModalOverlay className="preview-overlay" onBackdropClick={onClose}>
      <div className="preview-modal" onClick={(e) => e.stopPropagation()}>
        <button
          className="preview-close"
          onClick={onClose}
          type="button"
          aria-label="Close preview"
        >
          &times;
        </button>
        {/* Nav buttons only for single-select preview (image or PDF) */}
        {singleItem != null &&
          singlePreviewIndex != null &&
          singlePreviewIndex > 0 && (
            <button
              className="preview-nav preview-nav-left"
              onClick={() => onNavigate(singlePreviewIndex - 1)}
              type="button"
              aria-label="Previous image"
            >
              &#8249;
            </button>
          )}
        {singleItem != null &&
          singlePreviewIndex != null &&
          singlePreviewIndex < totalItems - 1 && (
            <button
              className="preview-nav preview-nav-right"
              onClick={() => onNavigate(singlePreviewIndex + 1)}
              type="button"
              aria-label="Next image"
            >
              &#8250;
            </button>
          )}

        {/* Single image preview */}
        {mode === "single-image" && layoutItems[0] && (
          <div className="preview-image-wrap">
            <img
              src={library.previewUrl(layoutItems[0], 1600)}
              alt={layoutItems[0].relPath}
            />
          </div>
        )}

        {/* Single video preview */}
        {mode === "single-video" && !videoError && videoUrl && (
          <div className="preview-video-wrap">
            <video
              key={videoUrl}
              src={videoUrl}
              controls
              autoPlay
              onError={() => setVideoError(true)}
              style={{ maxWidth: "100%", maxHeight: "80vh" }}
            />
          </div>
        )}

        {/* Video loading state */}
        {mode === "single-video" && !videoError && !videoUrl && (
          <div className="preview-video-wrap">
            <p style={{ color: "var(--text-secondary)" }}>Loading video...</p>
          </div>
        )}

        {/* Video error fallback: show thumbnail + message */}
        {mode === "single-video" && videoError && layoutItems[0] && (
          <div className="preview-video-wrap preview-video-fallback">
            {library.thumbUrl(layoutItems[0]) && (
              <img
                src={library.thumbUrl(layoutItems[0]) ?? undefined}
                alt={layoutItems[0].relPath}
                style={{ maxHeight: "50vh", objectFit: "contain" }}
              />
            )}
            <p style={{ color: "var(--text-secondary)", marginTop: 12 }}>
              Video playback not available. Your system may need GStreamer plugins
              (gst-plugins-good, gst-plugins-bad, gst-libav) for codec support.
            </p>
          </div>
        )}

        {/* Single PDF preview */}
        {mode === "single-pdf" && (
          <div className="preview-pdf-wrap">
            {pdfPane(pdfs[0])}
          </div>
        )}

        {/* Dual PDF side-by-side */}
        {mode === "dual-pdf" && (
          <div className="preview-split" data-testid="preview-split">
            <div className="preview-split-pane">
              {pdfPane(pdfs[0])}
            </div>
            <div className="preview-split-pane">
              {pdfPane(pdfs[1])}
            </div>
          </div>
        )}

        {/* PDF + image side-by-side */}
        {mode === "pdf-plus-image" && (
          <div className="preview-split" data-testid="preview-split">
            <div className="preview-split-pane">
              {pdfPane(pdfs[0])}
            </div>
            <div className="preview-split-pane preview-image-wrap">
              <img
                src={library.previewUrl(images[0], 1600)}
                alt={images[0].relPath}
              />
            </div>
          </div>
        )}

        {/* Image collage (2-10 images, no PDFs) */}
        {mode === "image-collage" && (
          <div className="preview-collage" data-count={images.length}>
            {images.map((item, idx) => (
              <div key={item.id} className="preview-collage-cell">
                <img
                  src={library.previewUrl(item, 1600)}
                  alt={item.relPath}
                />
                {images.length > 1 && (
                  <span className="preview-collage-label">{idx + 1}</span>
                )}
              </div>
            ))}
          </div>
        )}

        <div className="preview-info">
          {singleItem ? (
            <>
              <h3 title={singleItem.relPath}>
                {singleItem.relPath}
              </h3>
              <p className="preview-desc">
                {singleItem.description || "No description"}
              </p>
              <div className="preview-meta">
                <span className="badge">{singleItem.mediaType}</span>
                <span>
                  Confidence: {singleItem.confidence.toFixed(2)}
                </span>
                <span>
                  {formatBytes(singleItem.sizeBytes)}
                </span>
              </div>
            </>
          ) : previewItems.length <= 10 && singlePreviewIndex === null ? (
            <div className="preview-compare-list">
              {previewItems.map((item, idx) => (
                <div key={item.id} className="preview-compare-row">
                  <span className="preview-compare-num">{idx + 1}</span>
                  <span className="preview-compare-name" title={item.relPath}>
                    {fileName(item.relPath)}
                  </span>
                  <span className="preview-compare-meta">{formatBytes(item.sizeBytes)}</span>
                  <span className="preview-compare-meta">{item.mediaType}</span>
                  <span className="preview-compare-meta">
                    {new Date(item.mtimeNs / 1_000_000).toLocaleDateString()}
                  </span>
                </div>
              ))}
            </div>
          ) : (
            <h3>{selectedCount} files selected</h3>
          )}
        </div>
      </div>
    </ModalOverlay>
  );
}
