import { useCallback, useEffect, useRef, useState } from "react";

import type { SearchItem } from "../types";
import { useLibrary } from "../useLibrary";
import { errorMessage } from "../utils/errors";
import "./slideshow.css";

const BATCH_SIZE = 200;
export const DEFAULT_SLIDESHOW_SECONDS = 30;

type Props = {
  /** Seconds each photo stays on screen. Values outside 1..3600 are clamped. */
  intervalSeconds?: number;
  /** Extra class from the host shell (the web adds its flex sizing there). */
  className?: string;
};

function clampInterval(value: number | undefined): number {
  if (!Number.isFinite(value ?? NaN) || !value || value <= 0) {
    return DEFAULT_SLIDESHOW_SECONDS;
  }
  return Math.min(3600, Math.max(1, Math.round(value)));
}

/** Fisher-Yates; the backend returns a random batch, this only hides its order. */
function shuffled<T>(items: T[]): T[] {
  const copy = [...items];
  for (let i = copy.length - 1; i > 0; i -= 1) {
    const j = Math.floor(Math.random() * (i + 1));
    [copy[i], copy[j]] = [copy[j], copy[i]];
  }
  return copy;
}

/**
 * The portrait frame: one photo at a time, random order, no search or filters.
 *
 * The backend gives a random batch (`sortBy: "random"`, `imagesOnly: true`) so the
 * database is not asked for a fresh random row on every tick. The batch is
 * shuffled again here, which also lets the manual Previous button move backwards
 * through what was already shown. When a batch is exhausted a new one is loaded.
 */
export default function SlideshowView({ intervalSeconds, className }: Props) {
  const library = useLibrary();
  const interval = clampInterval(intervalSeconds);

  const [items, setItems] = useState<SearchItem[]>([]);
  const [index, setIndex] = useState(0);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [paused, setPaused] = useState(false);
  const [brokenIds, setBrokenIds] = useState<Set<number>>(new Set());

  const requestId = useRef(0);
  const lastShownId = useRef<number | null>(null);
  const current = items[index] ?? null;

  const loadBatch = useCallback(async () => {
    const id = ++requestId.current;
    setLoading(true);
    setError(null);
    try {
      const response = await library.search({
        query: "",
        limit: BATCH_SIZE,
        offset: 0,
        sortBy: "random",
        sortOrder: "desc",
        imagesOnly: true,
      });
      if (id !== requestId.current) return;

      // The query already excludes video and PDF rows. The defensive filter keeps a
      // deployment with an older backend from putting a PDF in the frame.
      const images = response.items.filter(
        (item) => item.mediaType !== "video" && item.mediaType !== "pdf",
      );
      if (images.length === 0) {
        setItems([]);
        setIndex(0);
        return;
      }

      let next = shuffled(images);
      if (lastShownId.current != null && next.length > 1 && next[0].id === lastShownId.current) {
        next = [...next.slice(1), next[0]];
      }
      setItems(next);
      setIndex(0);
      setBrokenIds(new Set());
    } catch (err) {
      if (id !== requestId.current) return;
      setError(errorMessage(err));
    } finally {
      if (id === requestId.current) setLoading(false);
    }
  }, [library]);

  useEffect(() => {
    void loadBatch();
  }, [loadBatch]);

  const goNext = useCallback(() => {
    if (items.length === 0) return;
    if (index >= items.length - 1) {
      void loadBatch();
    } else {
      setIndex((value) => value + 1);
    }
  }, [index, items.length, loadBatch]);

  const goPrev = useCallback(() => {
    setIndex((value) => (items.length === 0 ? 0 : value <= 0 ? items.length - 1 : value - 1));
  }, [items.length]);

  // Record what is on screen and fetch the next image before it is needed.
  const nextItem = items.length > 0 ? items[(index + 1) % items.length] : null;
  useEffect(() => {
    if (!current) return;
    lastShownId.current = current.id;
    if (!nextItem || typeof Image === "undefined") return;
    const preload = new Image();
    preload.src = library.previewUrl(nextItem, 2400);
  }, [current, nextItem, library]);

  // The timer is keyed by the current index, so a manual step does not wait out
  // the remainder of the previous photo.
  useEffect(() => {
    if (items.length === 0 || paused || loading) return;
    const timer = window.setTimeout(goNext, interval * 1000);
    return () => window.clearTimeout(timer);
  }, [items.length, index, interval, paused, loading, goNext]);

  // A background tab should not race through the library unseen.
  useEffect(() => {
    const update = () => setPaused(document.visibilityState === "hidden");
    update();
    document.addEventListener("visibilitychange", update);
    return () => document.removeEventListener("visibilitychange", update);
  }, []);

  useEffect(() => {
    function onKeyDown(event: KeyboardEvent) {
      const tag = (event.target as HTMLElement | null)?.tagName;
      if (tag === "INPUT" || tag === "SELECT" || tag === "TEXTAREA") return;
      if (event.key === "ArrowLeft") {
        event.preventDefault();
        goPrev();
      } else if (event.key === "ArrowRight") {
        event.preventDefault();
        goNext();
      }
    }
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [goNext, goPrev]);

  function handleImageError() {
    if (!current) return;
    const thumb = library.thumbUrl(current);
    if (thumb && !brokenIds.has(current.id)) {
      setBrokenIds((previous) => new Set(previous).add(current.id));
      return;
    }
    // A file the browser cannot decode must not freeze the frame forever.
    goNext();
  }

  const fallbackThumb = current ? library.thumbUrl(current) : null;
  const imageSrc = current
    ? brokenIds.has(current.id) && fallbackThumb
      ? fallbackThumb
      : library.previewUrl(current, 2400)
    : null;

  return (
    <section
      className={`slideshow${className ? ` ${className}` : ""}`}
      role="region"
      aria-label="Portrait mode slideshow"
    >
      {loading && items.length === 0 && (
        <div className="slideshow-state">Loading photos…</div>
      )}

      {!loading && error && (
        <div className="slideshow-state" role="alert">
          <span>Could not load the slideshow: {error}</span>
          <button type="button" onClick={() => void loadBatch()}>
            Try again
          </button>
        </div>
      )}

      {!loading && !error && items.length === 0 && (
        <div className="slideshow-state">
          <span>No photos available.</span>
          <button type="button" onClick={() => void loadBatch()}>
            Try again
          </button>
        </div>
      )}

      {current && imageSrc && (
        <>
          <img
            key={current.id}
            className="slideshow-image"
            src={imageSrc}
            alt={current.relPath}
            onError={handleImageError}
          />

          <button
            type="button"
            className="slideshow-nav slideshow-nav-prev"
            onClick={goPrev}
            aria-label="Previous photo"
          >
            &#8249;
          </button>
          <button
            type="button"
            className="slideshow-nav slideshow-nav-next"
            onClick={goNext}
            aria-label="Next photo"
          >
            &#8250;
          </button>

          <div className="slideshow-caption" title={current.relPath}>
            <span className="slideshow-name">{current.relPath}</span>
            <span className="slideshow-counter">
              {(index + 1).toLocaleString()} / {items.length.toLocaleString()}
            </span>
          </div>
        </>
      )}
    </section>
  );
}
