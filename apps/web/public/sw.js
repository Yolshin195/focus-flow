// Service worker веб-демо Focus Flow. Область — каталог сайта (/focus-flow/).
//
// - Переход на любой экран приложения (в том числе /tasks, который Pages
//   отдаёт как 404.html) — index.html из кэша сразу, свежий — в фоне
//   (stale-while-revalidate).
// - Файлы с хэшем в имени (assets/*-dxh….wasm|js) и шрифты не меняются —
//   из кэша, иначе из сети с сохранением.
// - Остальное в каталоге сайта (main.css, иконки, manifest) — из кэша сразу,
//   свежее — в фоне.
//
// Версия сборки — хэши wasm и js в index.html. Пришёл другой index.html —
// SW сначала докачивает файлы новой сборки (без хэша — с перепроверкой),
// затем кладёт новый index.html и удаляет файлы старой: следующий запуск —
// уже новая версия, целиком из кэша. Поменялись шрифты — поднять CACHE.

const CACHE = "focus-flow-v1";
const SCOPE = new URL(self.registration.scope).pathname;
const INDEX = SCOPE;
const HASHED = /-dxh[0-9a-f]+\.[a-z0-9]+$/;
const FONT = /\/assets\/fonts\/[^/]+\.woff2$/;
const FILE = new RegExp(
  '"(' + SCOPE.replace(/[.*+?^${}()|[\]\\]/g, "\\$&") + '[^"?#]+\\.(?:wasm|js|css|woff2|svg|png|webmanifest))"',
  "g",
);

/** Файлы сборки, на которые ссылается index.html. */
function buildFiles(html) {
  return [...new Set([...html.matchAll(FILE)].map((m) => m[1]))];
}

/** Поставить сборку из `html` (ответ `response`) вместо текущей. */
async function installBuild(html, response) {
  const cache = await caches.open(CACHE);
  const files = buildFiles(html);
  await Promise.all(
    files.map(async (url) => {
      const fixed = HASHED.test(url) || FONT.test(url);
      if (fixed && (await cache.match(url))) return;
      const res = await fetch(url, { cache: fixed ? "default" : "no-cache" });
      if (res.ok) await cache.put(url, res);
    }),
  );
  await cache.put(INDEX, response);
  const keep = new Set([INDEX, ...files].map((f) => new URL(f, self.location.origin).href));
  for (const req of await cache.keys()) {
    if (!keep.has(req.url)) await cache.delete(req);
  }
}

/** Проверить index.html на сайте; новый — поставить. */
async function refresh() {
  const res = await fetch(INDEX, { cache: "no-cache" });
  if (!res.ok) return;
  const html = await res.clone().text();
  const cached = await caches.match(INDEX);
  if (cached && (await cached.text()) === html) return;
  await installBuild(html, res);
}

/** Из кэша сразу, в фоне — свежая копия в кэш. */
function staleWhileRevalidate(event, key) {
  const fresh = fetch(event.request.url, { cache: "no-cache" }).then(async (res) => {
    if (res.ok) await (await caches.open(CACHE)).put(key, res.clone());
    return res;
  });
  event.waitUntil(fresh.catch(() => {}));
  return caches.match(key).then((hit) => hit || fresh);
}

self.addEventListener("install", (event) => {
  event.waitUntil(refresh().then(() => self.skipWaiting()));
});

self.addEventListener("activate", (event) => {
  event.waitUntil(
    (async () => {
      for (const key of await caches.keys()) {
        if (key.startsWith("focus-flow-") && key !== CACHE) await caches.delete(key);
      }
      await self.clients.claim();
    })(),
  );
});

self.addEventListener("fetch", (event) => {
  const req = event.request;
  if (req.method !== "GET") return;
  const url = new URL(req.url);
  if (url.origin !== self.location.origin || !url.pathname.startsWith(SCOPE)) return;
  if (url.pathname === SCOPE + "sw.js") return;

  // Экраны приложения — пути без расширения; файлы (og.png и т.п.) — как есть.
  if (req.mode === "navigate") {
    if (/\.[a-z0-9]+$/i.test(url.pathname)) return;
    event.waitUntil(refresh().catch(() => {}));
    event.respondWith(caches.match(INDEX).then((hit) => hit || fetch(req)));
    return;
  }

  if (HASHED.test(url.pathname) || FONT.test(url.pathname)) {
    event.respondWith(
      caches.match(url.href).then(
        (hit) =>
          hit ||
          fetch(req).then(async (res) => {
            if (res.ok) await (await caches.open(CACHE)).put(url.href, res.clone());
            return res;
          }),
      ),
    );
    return;
  }

  event.respondWith(staleWhileRevalidate(event, url.href));
});
