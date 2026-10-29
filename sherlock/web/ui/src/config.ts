import { createHttpLibraryClient } from "@frank-sherlock/shared";

const TOKEN_KEY = "frank-sherlock-token";

/**
 * Token of the deployment, from `?token=` or from the previous visit.
 *
 * The query form exists because a phone cannot set a header on the first navigation;
 * storing it means the next visit does not need it in the URL (which ends up in logs).
 */
export function resolveToken(
  search: string = window.location.search,
  storage: Storage = window.localStorage,
): string {
  const fromUrl = new URLSearchParams(search).get("token");
  if (fromUrl) {
    saveToken(fromUrl, storage);
    return fromUrl;
  }
  try {
    return storage.getItem(TOKEN_KEY) ?? "";
  } catch {
    return "";
  }
}

/** Remember a token the server accepted, so the login form is not needed again. */
export function saveToken(token: string, storage: Storage = window.localStorage): void {
  try {
    storage.setItem(TOKEN_KEY, token);
  } catch {
    /* private mode: the token still works for this session */
  }
}

/** Forget the stored token (it was rejected, or the user wants a different one). */
export function clearToken(storage: Storage = window.localStorage): void {
  try {
    storage.removeItem(TOKEN_KEY);
  } catch {
    /* nothing to clear when storage is unavailable */
  }
}

/** Same-origin client: the binary that served this bundle also serves `/api`. */
export function createWebClient(
  token: string = resolveToken(),
  onUnauthorized?: () => void,
): ReturnType<typeof createHttpLibraryClient> {
  return createHttpLibraryClient({ token, onUnauthorized });
}

/** Drop `?token=` from the address bar once it was stored. */
export function stripTokenFromUrl(): void {
  const url = new URL(window.location.href);
  if (!url.searchParams.has("token")) return;
  url.searchParams.delete("token");
  window.history.replaceState(null, "", `${url.pathname}${url.search}${url.hash}`);
}
