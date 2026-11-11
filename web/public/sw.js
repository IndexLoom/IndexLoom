/* Offline static shell only. NEVER cache API, OAuth, credentials or responses. */
const CACHE_NAME = "indexloom-shell-v2";
const PRECACHE = ["/", "/manifest.webmanifest", "/icons/indexloom.svg"];
self.addEventListener("install", (event) => {
  event.waitUntil(caches.open(CACHE_NAME).then((cache) => cache.addAll(PRECACHE)));
  self.skipWaiting();
});
self.addEventListener("activate", (event) => {
  event.waitUntil(
    caches.keys().then((keys) => Promise.all(
      keys.filter((key) => key.startsWith("indexloom-") && key !== CACHE_NAME)
        .map((key) => caches.delete(key))
    )).then(() => self.clients.claim())
  );
});
self.addEventListener("fetch", (event) => {
  const request = event.request;
  const url = new URL(request.url);
  if (request.method !== "GET" || url.origin !== self.location.origin
      || url.pathname.startsWith("/api/") || url.pathname.startsWith("/auth/")
      || request.headers.has("Authorization")) return;
  if (request.mode === "navigate") {
    event.respondWith(
      fetch(request).then((response) => {
        if (response.ok) {
          const copy = response.clone();
          caches.open(CACHE_NAME).then((cache) => cache.put("/", copy));
        }
        return response;
      }).catch(async () =>
        (await caches.match("/")) ||
        new Response("IndexLoom is offline. Reconnect to sign in or search.", {
          status: 503, headers: { "Content-Type": "text/plain;charset=utf-8" },
        })
      )
    );
    return;
  }
  if (url.pathname.startsWith("/assets/")
      || url.pathname.startsWith("/icons/")
      || url.pathname === "/manifest.webmanifest") {
    event.respondWith(
      caches.match(request).then((cached) => cached || fetch(request).then((response) => {
        if (response.ok) {
          const copy = response.clone();
          caches.open(CACHE_NAME).then((cache) => cache.put(request, copy));
        }
        return response;
      }))
    );
  }
});
