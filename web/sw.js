// The service worker: lets gase start without a network connection, and
// makes it installable ("Add to Home Screen").
//
// Strategy: network first, cache as fallback. Online, every load gets the
// current files (with the browser's usual HTTP caching), so an update
// never mixes an old gase.js with a new gase_web.wasm; each response is
// also copied into the cache. Offline, the cached copies are served. The
// page shell is cached at install time so the first offline start works
// even if some files were never requested.

const CACHE = 'gase-shell';
const SHELL = [
  './',
  'index.html',
  'gase.css',
  'gase.js',
  'input.js',
  'audio.js',
  'audio-worklet.js',
  'storage.js',
  'gase_web.wasm',
  'manifest.webmanifest',
  'icons/icon.svg',
  'icons/icon-192.png',
  'icons/icon-512.png',
  'icons/maskable-512.png',
  'icons/apple-touch-icon.png',
];

self.addEventListener('install', (event) => {
  event.waitUntil(
    caches
      .open(CACHE)
      .then((cache) => cache.addAll(SHELL))
      .then(() => self.skipWaiting()),
  );
});

self.addEventListener('activate', (event) => {
  event.waitUntil(self.clients.claim());
});

self.addEventListener('fetch', (event) => {
  const request = event.request;
  // Only this site's files; the test ROM download goes straight out.
  if (request.method !== 'GET' || new URL(request.url).origin !== location.origin) return;
  event.respondWith(
    (async () => {
      const cache = await caches.open(CACHE);
      try {
        const response = await fetch(request);
        if (response.ok) event.waitUntil(cache.put(request, response.clone()));
        return response;
      } catch (offline) {
        const cached = await cache.match(request, { ignoreSearch: true });
        if (cached) return cached;
        throw offline;
      }
    })(),
  );
});
