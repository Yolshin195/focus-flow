# Focus Flow

Трекер фокуса 50/10 на Rust: локальный сервис (Axum + SQLite) + клиенты на Dioxus 0.7 (desktop сейчас; web и mobile — заготовки) + MCP + плагин Jira.

- Спецификация: `docs/SPEC.md` — источник истины. План сервиса: `docs/PLAN-backend.md`. Решения: `docs/DECISIONS.md`. Запросы на изменение контракта: `docs/CONTRACT-REQUESTS.md`.
- Правила для клиентов и официальные инструкции Dioxus — в `apps/CLAUDE.md` (подгружаются при работе в `apps/`).
- Общение, коммиты, docs — на русском; идентификаторы в коде — на английском.

## Структура

```
crates/  api-types api-contract-tests core storage plugin-api plugin-jira api-rest api-mcp server   — сервис (пакеты focus-*)
apps/    ui desktop web mobile                                                                     — клиенты Dioxus (пакеты ui, desktop, web, mobile)
bin/     focus-flow                                                                                — CLI: serve | ui | token
```

## Владение кодом (две параллельные сессии)

| Сессия | Пишет в | Ветка |
|---|---|---|
| A — сервис | `crates/`, `bin/`, `docs/`, корневой `Cargo.toml` | `main` |
| B — UI | только `apps/` | `ui` (отдельный worktree) |

- `crates/api-types` — общий контракт клиента и сервиса. Меняет только сессия A. Сессии B нужно изменение → запись в `docs/CONTRACT-REQUESTS.md` (что, зачем, пример) и пока работает на временном типе внутри `apps/ui`.
- Новую зависимость для `apps/*` сессия B добавляет в `Cargo.toml` своего крейта с явной версией; в `[workspace.dependencies]` её переносит сессия A.

## Архитектурные правила (нарушать нельзя)

1. `apps/*` зависят только от `apps/ui` и `crates/api-types`. Никаких `focus-core`, `focus-storage`, `sqlx`, серверного `tokio` в клиентах.
2. `api-types` собирается под `wasm32-unknown-unknown`: только `serde`, `chrono`, `uuid`, `async-trait`, `thiserror`. Без логики — типы, трейт `FocusApi`, `ApiError`, события.
3. `core` не знает про SQL, HTTP, Axum, Jira. Внешний мир — через трейты (порты): `Clock`, репозитории, шина событий. Реализации подставляет `server`.
4. Ядро не знает про конкретные плагины. Плагины реагируют на `DomainEvent` и подключаются в `server` через реестр и cargo features.
5. Поведение клиента и сервиса сверяется сценариями `api-contract-tests`: они обязаны проходить и на `MockApi`, и на сервисе.

## Качество

- Перед коммитом: `cargo fmt --all`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`. Для `apps/`: ещё `dx build` в `apps/desktop`.
- Один шаг плана или один UI-цикл = один коммит: «шаг N: …» или «ui: …».
- Ошибки: `thiserror` в библиотеках, `anyhow` только в `server` и `bin`. Без `unwrap()` вне тестов.
- Логи: `tracing`. Время в домене — `chrono::DateTime<Utc>`; границы дня — в локальном поясе системы.
- Секретов в репозитории нет: токены — через `keyring` или переменные окружения.
- Если решения нет в SPEC — принять разумное, записать в `docs/DECISIONS.md` и продолжить.
