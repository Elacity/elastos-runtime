const CACHE_NAME = "elastos-home-20260721b";
const CACHE_PREFIX = "elastos-home-";

self.addEventListener("install", (event) => {
  event.waitUntil(self.skipWaiting());
});

self.addEventListener("activate", (event) => {
  event.waitUntil(
    caches.keys()
      .then((keys) => Promise.all(
        keys
          .filter((key) => key.startsWith(CACHE_PREFIX))
          .map((key) => caches.delete(key)),
      ))
      .then(() => self.clients.claim())
      .then(() => self.registration.unregister()),
  );
});

self.addEventListener("fetch", () => {});
