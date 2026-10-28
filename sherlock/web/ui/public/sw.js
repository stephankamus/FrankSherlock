/*
 * Minimal app-shell cache: the API is always network-only (it is the library), and
 * the shell is cached so the PWA opens on a phone even before the first request
 * reaches the LAN share. There is no offline library by design.
 */
const CACHE = "frank-sherlock-shell-v1";
const SHELL = ["/", "/manifest.webmanifest", "/icon-128.png", "/icon-512.png"];

self.addEventListener("install", (event) => {
  event.waitUntil(
    caches
      .open(CACHE)
      .then((cache) => cache.addAll(SHELL))
      .then(() => self.skipWaiting()),
  );
});

self.addEventListener("activate", (event) => {
  event.waitUntil(self.clients.claim());
});

self.addEventListener("fetch", (event) => {
  const url = new URL(event.request.url);
  if (event.request.method !== "GET" || url.pathname.startsWith("/api/")) return;
  event.respondWith(
    caches.match(event.request).then((hit) => hit || fetch(event.request)),
  );
});
