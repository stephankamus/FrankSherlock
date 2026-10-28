import React from "react";
import ReactDOM from "react-dom/client";
import { LibraryProvider } from "@frank-sherlock/shared";
import App from "./App";
import { tauriLibraryClient } from "./tauriLibraryClient";
import "./theme.css";

ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <LibraryProvider client={tauriLibraryClient}>
      <App />
    </LibraryProvider>
  </React.StrictMode>
);
