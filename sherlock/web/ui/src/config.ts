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
    try {
      storage.setItem(TOKEN_KEY, fromUrl);
    } catch {
      /* private mode: the URL value still works for this session */
    }
    return fromUrl;
  }
  try {
    return storage.getItem(TOKEN_KEY) ?? "";
  } catch {
    return "";
  }
}

/** Same-origin client: the binary that served this bundle also serves `/api`. */
export function createWebClient(): ReturnType<typeof createHttpLibraryClient> {
  return createHttpLibraryClient({ token: resolveToken() });
}

/** Drop `?token=` from the address bar once it was stored. */
export function stripTokenFromUrl(): void {
  const url = new URL(window.location.href);
  if (!url.searchParams.has("token")) return;
  url.searchParams.delete("token");
  window.history.replaceState(null, "", `${url.pathname}${url.search}${url.hash}`);
}
