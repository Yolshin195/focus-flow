# UI (apps/)

Официальные инструкции Dioxus 0.7 лежат в `AGENTS.md` рядом с этим файлом. Не редактировать — обновлять только заменой файла целиком (свежая версия: `dx new` с включёнными LLM prompts или сайт Dioxus):
@AGENTS.md

## Уточнения для этого проекта (приоритет над AGENTS.md)

- **Fullstack не используем.** Сервер — отдельный Axum-сервис. Features `server`/`fullstack`, `#[post]`/`#[get]`, `use_server_future` не нужны; данные — только через трейт `FocusApi` из `focus-api-types`.
- **Экраны общие.** `enum Route`, экраны («Фокус», «Задачи», «День», «Настройки»), layout и компоненты живут в `apps/ui`. Платформенный крейт (`apps/desktop` и др.) содержит только точку входа: реализацию `Platform`, выбор `FocusApi` (`MockApi`/`HttpApi`) и запуск `ui::App`. У `desktop` это `lib.rs` (`launch`, его вызывает и `focus-flow ui`) и тонкий `main.rs` для `dx`. Свой экран у платформы — только как осознанное исключение.
- **`ui` не знает про платформу.** Всё платформенное (уведомления, wake lock, адрес сервиса) — через трейт `Platform` из контекста (`use_context_provider` в `main.rs`, `use_context` в компонентах). Стилевые различия — CSS-класс `platform-desktop|web|mobile` на корне.
- **Зависимости `ui`:** `dioxus` (с `router`), `focus-api-types` и кроссплатформенное для `HttpApi` (`reqwest` без TLS, `serde`, `futures-*`, `async-trait`) — все из `[workspace.dependencies]`. Платформенные крейты (`notify-rust`, `web-sys` и т.п.) — только в крейтах платформ.
- **Компоненты не ходят в HTTP напрямую** — только через `FocusApi`, полученный из контекста.
- **Стили:** `document::Stylesheet` + `asset!()`; дизайн-токены (цвета, отступы, радиусы, шрифты) — в `apps/ui/assets/tokens.css`; тёмная тема по умолчанию. Цвета сессий: работа — синий, отдых — зелёный, планирование — жёлтый, потерянное время — серый.
- **Сейчас только desktop.** `apps/web` держим собираемым, но не полируем; `apps/mobile` исключён из workspace до появления мобильных toolchain'ов.
- **Проверка перед коммитом:** `cargo clippy -p ui -p desktop -- -D warnings`, `cd apps/desktop && dx build`.
