import React from "react";
import ReactDOM from "react-dom/client";
import "@frank-sherlock/shared/theme.css";
import "./app.css";
import TokenGate from "./TokenGate";

ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <TokenGate />
  </React.StrictMode>,
);

// The service worker is only useful in the built app; registering it in dev makes
// Vite's HMR fight the cache.
if (import.meta.env.PROD && "serviceWorker" in navigator) {
  window.addEventListener("load", () => {
    void navigator.serviceWorker.register("/sw.js").catch(() => {
      /* the app works without it */
    });
  });
}
