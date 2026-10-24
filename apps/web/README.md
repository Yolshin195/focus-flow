# web — демо в браузере

Тот же `ui::App` с `MockApi`, без сервера. Публикуется на GitHub Pages
(`.github/workflows/pages.yml`): https://yolshin195.github.io/focus-flow/

```sh
cargo check -p web --target wasm32-unknown-unknown
cd apps/web && dx bundle --platform web --release --debug-symbols=false
```

`--debug-symbols=false` обязателен: у dx 0.7 он по умолчанию включён и в
release, и в wasm остаётся DWARF (~1.4 МБ). Профиль `wasm-release` из корневого
`Cargo.toml` dx берёт сам.

## Первый кадр без скачков

- `index.html` — свой шаблон dx: `{base_path}`, `{wasm_path}`, `{app_title}`
  (заголовок — в `Dioxus.toml`), `{style_include}` (сюда dx вставляет preload JS).
- Внутри `#main` — статичный каркас: первый кадр `web::Root` до прихода данных.
  Его генерирует и сверяет тест `tests/skeleton.rs`; после правок оболочки или
  экрана «Фокус»: `UPDATE_SKELETON=1 cargo test -p web --test skeleton`.
- `<style id="critical">` — дословные правила из `apps/ui/assets/tokens.css` и
  `main.css` для каркаса (тот же тест проверяет, что они не разошлись).
  `<style id="boot-style">` — только для веба: `@font-face` (`font-display: swap`),
  системные fallback-шрифты с метриками Inter/JetBrains Mono, индикатор загрузки.
- `main.css` подключается `<link>` без блокировки первого кадра; `ui::App` на
  вебе стили и шрифты не вставляет (`Platform::styles_in_page`).
- `web::Root` первым делом убирает каркас (`#boot`) — в том же JS-задании, в
  котором Dioxus вставляет свой первый кадр.

## PWA

`public/` копируется в корень сайта без хэшей: иконки, `manifest.webmanifest`,
`sw.js`. Service worker (область `/focus-flow/`) регистрируется после запуска
приложения: index.html — stale-while-revalidate, файлы с хэшем и шрифты — из
кэша. Новая сборка на сайте докачивается в фоне и открывается при следующем
запуске; файлы прошлой сборки удаляются.
