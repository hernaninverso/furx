// Minimal offline shell for the Furx Mobile PWA. Caches the static app files so
// the UI loads without the desktop reachable; the live data is always over WS
// (never cached). Bump CACHE on any asset change.
// Bump to v2: 017 added the bottom-nav modules to the offline shell.
const CACHE = "furx-mobile-v2";
const ASSETS = [
  "./", "index.html", "furx-sign.js", "manifest.webmanifest", "icon.svg",
  // 017 — data-driven bottom-nav modules.
  "protocol.js", "nav.js", "commands.js", "events.js",
];

self.addEventListener("install", (e) => {
  e.waitUntil(caches.open(CACHE).then((c) => c.addAll(ASSETS)).then(() => self.skipWaiting()));
});

self.addEventListener("activate", (e) => {
  e.waitUntil(
    caches.keys().then((keys) =>
      Promise.all(keys.filter((k) => k !== CACHE).map((k) => caches.delete(k))),
    ).then(() => self.clients.claim()),
  );
});

self.addEventListener("fetch", (e) => {
  const url = new URL(e.request.url);
  // Never intercept the WebSocket upgrade or API paths.
  if (url.pathname.startsWith("/ws") || e.request.method !== "GET") return;
  e.respondWith(
    caches.match(e.request).then((hit) => hit || fetch(e.request)),
  );
});
