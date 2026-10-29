import { useEffect, useRef, useState } from "react";
import { loadUserConfig, saveUserConfig } from "../api";

export function useUserConfig() {
  const [zoom, setZoom] = useState(1.25);
  /**
   * "Show hidden" filter, persisted with the rest of the UI config.
   *
   * Default off: hidden photos must stay hidden after a restart, not only until the
   * window is reopened.
   */
  const [showHidden, setShowHiddenState] = useState(false);
  const configRef = useRef<Record<string, unknown>>({});

  // Load user config (zoom + visibility filter) on mount
  useEffect(() => {
    let mounted = true;
    loadUserConfig()
      .then((cfg) => {
        if (!mounted) return;
        configRef.current = cfg;
        const savedZoom = typeof cfg.zoom === "number" ? cfg.zoom : 1.25;
        setZoom(Math.max(0.5, Math.min(3.0, savedZoom)));
        setShowHiddenState(cfg.showHidden === true);
      })
      .catch(() => {});
    return () => { mounted = false; };
  }, []);

  // Apply zoom to root font-size
  useEffect(() => {
    document.documentElement.style.fontSize = `${14 * zoom}px`;
  }, [zoom]);

  // Keyboard: Ctrl+Shift+= (zoom in), Ctrl+Shift+- (zoom out)
  useEffect(() => {
    function handleZoomKey(e: KeyboardEvent) {
      if (!e.ctrlKey || !e.shiftKey) return;
      if (e.key === "+" || e.key === "=") {
        e.preventDefault();
        setZoom((prev) => {
          const next = Math.min(3.0, +(prev + 0.1).toFixed(2));
          persistZoom(next);
          return next;
        });
      } else if (e.key === "-" || e.key === "_") {
        e.preventDefault();
        setZoom((prev) => {
          const next = Math.max(0.5, +(prev - 0.1).toFixed(2));
          persistZoom(next);
          return next;
        });
      }
    }
    window.addEventListener("keydown", handleZoomKey);
    return () => window.removeEventListener("keydown", handleZoomKey);
  }, []);

  function persistZoom(value: number) {
    const cfg = { ...configRef.current, zoom: value };
    configRef.current = cfg;
    saveUserConfig(cfg).catch(() => {});
  }

  const setShowHidden = (value: boolean) => {
    setShowHiddenState(value);
    const cfg = { ...configRef.current, showHidden: value };
    configRef.current = cfg;
    saveUserConfig(cfg).catch(() => {});
  };

  return { zoom, showHidden, setShowHidden };
}
