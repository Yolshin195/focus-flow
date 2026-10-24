# План: сервис (ядро → REST → MCP → Jira)

Выполняет сессия A. Каждый шаг — отдельный субагент со свежим контекстом. После шага: `cargo fmt`, `cargo clippy --all-targets -- -D warnings`, `cargo test` зелёные → коммит → отметка `[x]` здесь → следующий шаг.

- [x] **0. Каркас сервиса и контракт.** В репозитории уже есть `apps/ui|desktop|web|mobile` из шаблона dx Workspace (пакеты `ui`, `desktop`, `web`, `mobile`). `apps/` не трогать.
    1. Создать `crates/{api-types,api-contract-tests,core,storage,plugin-api,plugin-jira,api-rest,api-mcp,server}` (`cargo new --lib --vcs none --name focus-<имя>`) и `bin/focus-flow` (`cargo new --vcs none --name focus-flow`). Пустые `lib.rs` — с doc-комментарием о назначении.
    2. Корневой `Cargo.toml`: `resolver = "3"`; `members = ["apps/ui", "apps/desktop", "apps/web", "crates/*", "bin/*"]` (`apps/mobile` — в `exclude`, пока нет мобильных toolchain'ов); `[workspace.package]` (edition 2024, version 0.1.0, license MIT) для новых крейтов; в `[workspace.dependencies]` к существующим `dioxus` и `ui` добавить `focus-api-types = { path = "crates/api-types" }` и общие версии (axum, tokio, sqlx с sqlite, reqwest, rmcp, async-trait, thiserror, anyhow, tracing, clap, serde, chrono, uuid). Новые крейты берут зависимости через `{ workspace = true }`.
    3. `focus-api-types` — контракт клиента и сервиса (только serde, chrono, uuid, async-trait, thiserror):
        - все DTO из SPEC; недопустимые состояния невыразимы (`enum ActiveSession { Work { item, started_at, ends_at }, Rest { started_at, ends_at }, Planning { started_at } }`, а не строка `kind` + `Option`);
        - `enum ApiError` (`NoActiveSession`, `SessionAlreadyActive`, `ItemRequired`, `ItemNotFound`, `DayClosed`, `PluginUnavailable(String)`, `Validation(String)`, `Internal(String)`) с сериализацией;
        - трейт `FocusApi` (`#[async_trait(?Send)]`, по методу на каждую строку таблицы REST API в SPEC) + поток событий SSE (`ServerEvent`);
        - правила совместимости: новые поля с `#[serde(default)]`, у enum событий вариант `#[serde(other)] Unknown`.
        - проверка: `cargo check -p focus-api-types --target wasm32-unknown-unknown`.
    4. Крейт `crates/api-contract-tests` (`focus-api-contract-tests`): сценарии поведения, написанные против `&dyn FocusApi` как обычные async-функции (не `#[test]`), чтобы их могли вызвать и UI-мок, и сервер. Минимум: старт работы автооткрывает день; работа без пункта → `ItemRequired`; «Готово» → пункт done и выбор rest/planning; «Прервать» → `end_reason = interrupted`; повторный старт при активной сессии → `SessionAlreadyActive`; планирование идёт в норму; отдых — нет.
    5. `bin/focus-flow`: clap с подкомандами `serve` и `ui` (пока заглушки).
    6. `rust-toolchain.toml` (stable), `docs/DECISIONS.md`, `docs/CONTRACT-REQUESTS.md` (пустые с заголовком). Дописать в `.gitignore`: `*.db`, `.env`, `.DS_Store`.
    7. Проверка: `cargo check --workspace` и `cd apps/desktop && dx build` — всё зелёное.
    Сразу коммит «шаг 0: каркас сервиса и контракт» и `git push` — сессия B ждёт этот шаг.
- [x] **1. Ядро: домен и правила.** `core`: сущности, машина состояний сессий, все правила таймера из SPEC (без паузы, interrupt, done → rest|planning, лимит, норма, потерянное время, автооткрытие дня, полночь). Время через трейт `Clock` (тесты с фейковыми часами). Покрыть правила unit-тестами.
- [ ] **2. Хранилище.** `storage`: sqlx + SQLite, миграции, репозитории; дерево задач; outbox. Интеграционные тесты на временной БД.
- [ ] **3. Сервис и таймер.** Крейт `server`, фоновая задача: следит за `ends_at`, закрывает сессию по лимиту, стартует отдых, шлёт системное уведомление (`notify-rust`) и событие в шину. Конфиг, data dir (`directories`), токен.
- [ ] **4. REST + SSE.** `api-rest` по таблице SPEC; Bearer-токен, проверка Host/Origin; SSE `/api/events`. Тесты через `axum::Router` + `tower::ServiceExt`. Обязательно: прогнать все сценарии `focus-api-contract-tests` против сервиса (тестовая реализация `FocusApi` поверх роутера) — все зелёные.
- [ ] **5. Плагинная система.** `plugin-api`: трейты, `DomainEvent`, `PluginCtx`; реестр плагинов; воркер outbox с повторами; `/api/plugins/status`; `/api/import` через `TaskSource`. Тестовый фейковый плагин.
- [ ] **6. MCP.** `api-mcp` на `rmcp`, Streamable HTTP на `/mcp` в том же Axum; инструменты из SPEC + `mcp_tools()` плагинов. Проверить: `claude mcp add --transport http focus-flow http://127.0.0.1:7878/mcp --header "Authorization: Bearer <token>"`. В README — инструкция подключения.
- [ ] **7. Плагин Jira.** Всё из раздела SPEC «Плагин Jira»: настройки (base URL, режим DC/Cloud, токен в keyring, ID поля с автопоиском), импорт, синхронизация пунктов, worklog, сопоставление строк по тексту с перечитыванием. Тесты на `wiremock`.
- [ ] **8. Финиш.** README: сборка, запуск, подключение Claude, настройка Jira. Скрипт/инструкция автозапуска `serve` (launchd на macOS). Прогнать всё, исправить предупреждения.

Если в шаге нужно решение, которого нет в SPEC: принять разумное, записать в `docs/DECISIONS.md` (дата, шаг, решение, почему) и продолжить. Не останавливаться с вопросом.
