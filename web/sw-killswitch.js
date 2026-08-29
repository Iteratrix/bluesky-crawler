// Kill-switch service worker. Deploy this manually as `sw.js` to recover from
// a broken SW that's bricked the site for cached clients.
//
// What it does:
//   1. Unregisters itself on install.
//   2. Deletes every cache.
//   3. Reloads any controlled clients so they fetch the network version.
//
// Once deployed and clients have picked it up, replace it with the real sw.js.

self.addEventListener("install", (event) => {
  event.waitUntil(self.skipWaiting());
});

self.addEventListener("activate", (event) => {
  event.waitUntil((async () => {
    const keys = await caches.keys();
    await Promise.all(keys.map((k) => caches.delete(k)));
    await self.registration.unregister();
    const clients = await self.clients.matchAll({ type: "window" });
    for (const client of clients) {
      client.navigate(client.url);
    }
  })());
});
