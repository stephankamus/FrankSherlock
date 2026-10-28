import { createContext, useContext, type ReactNode } from "react";

import type { LibraryCapabilities, LibraryClient } from "./client";

const LibraryClientContext = createContext<LibraryClient | null>(null);

type LibraryProviderProps = {
  client: LibraryClient;
  children: ReactNode;
};

/**
 * Makes one client available to every shared view.
 *
 * Both applications mount exactly one provider: the desktop with the Tauri client,
 * the web with the HTTP one. A view that reads `useLibrary()` therefore works in
 * both, and the capabilities of the deployment are always one property away.
 */
export function LibraryProvider({ client, children }: LibraryProviderProps) {
  return (
    <LibraryClientContext.Provider value={client}>{children}</LibraryClientContext.Provider>
  );
}

/**
 * The client of the current application.
 *
 * Throws instead of falling back to Tauri: a web build that forgot the provider
 * should fail loudly, not silently try to call `invoke`.
 */
export function useLibrary(): LibraryClient {
  const client = useContext(LibraryClientContext);
  if (!client) {
    throw new Error(
      "useLibrary() must be used inside <LibraryProvider client={...}>: mount the " +
        "Tauri client in the desktop and the HTTP client in the web app.",
    );
  }
  return client;
}

export function useLibraryCapabilities(): LibraryCapabilities {
  return useLibrary().capabilities;
}
