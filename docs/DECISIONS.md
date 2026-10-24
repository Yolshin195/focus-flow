# Решения

Формат: дата · шаг · решение — почему.

## 2026-10-24 · шаг 0

- **В работу берётся только пункт — узел без детей.** Задача с пунктами → `ApiError::ItemRequired`; несуществующий id → `ItemNotFound`. Узел верхнего уровня без детей — тоже пункт (SPEC: «пункт без родителя допустим»). Иначе время на задаче с пунктами неясно куда относить (Jira worklog — на задачу, комментарий — текст пункта).
- **`ApiError` расширен** сверх списка плана: `WrongSessionKind { expected, actual }` («Стоп» во время работы, «Прервать» во время отдыха/планирования — это не «нет сессии» и не «уже активна»), `Unauthorized` (Bearer-токен), `Transport(String)` (создаёт только клиент `HttpApi` при сетевой ошибке) и `Unknown` (`#[serde(other)]`, для новых кодов от нового сервиса). JSON: `{"code": "...", "details": ...}`. HTTP: 409 — конфликты состояния (`NoActiveSession`, `SessionAlreadyActive`, `WrongSessionKind`, `DayClosed`), 404 — `ItemNotFound`, 422 — `ItemRequired`, `Validation`, 401, 503 — плагин, 500 — внутренняя.
- **`ActiveSession` несёт `session_id`**, у `Work` — `ItemRef` (id, название, родитель), у `Planning` нет `ends_at`. JSON: тег `kind`.
- **«Готово», «Прервать», «Стоп» возвращают `SessionEndResult { ended, status }`** — клиенту сразу видны закрытая сессия (длительность, `end_reason`) и новое состояние (например, уже идущий отдых). Старты и операции с днём возвращают `StatusDto`.
- **Планирование закрывается стартом работы или отдыха** (`end_reason = switched`), а не ошибкой; работа и отдых не прерываются стартом другой сессии (`SessionAlreadyActive`).
- **`close_day` при активной сессии → `SessionAlreadyActive`.** `open_day` по закрытому сегодняшнему дню открывает его снова (тот же `DayId`, `closed_at = null`). Старт сессии в закрытый день → `DayClosed`.
- **`StatusDto.server_time`** — клиент считает таймер от времени сервиса, а не от своих часов. `idle_since` — начало текущего потерянного времени.
- **`TaskPatch`**: отсутствующее поле — не менять, `null` — очистить (`Option<Option<T>>` для `notes`, `planned_for`, `estimate_intervals`).
- **Настройки длин — в минутах (`*_min`), всё остальное — в секундах (`*_sec`)**; настройки плагинов — `BTreeMap<String, serde_json::Value>`, схему знает плагин; секреты не возвращаются. Поэтому `serde_json` в зависимостях `api-types` (собирается в wasm).
- **wasm: `uuid` с `js`, `chrono` с `wasmbind`** — только для `wasm32-unknown-unknown`, чтобы `Uuid::now_v7()` и `Utc::now()` работали в браузерном `MockApi`.
- **События SSE** — только тип `ServerEvent` (`{"type", "data"}`, `event:` = `ServerEvent::name()`); метода подписки в `FocusApi` нет, чтобы не тянуть `futures`/runtime в контракт. Клиент подписывается на `/api/events` сам.
- **Сценарии контракта** (`focus-api-contract-tests`) требуют свежий бэкенд; сценарий, оставляющий активный отдых (его нельзя пропустить), идёт в `run_all` последним. С управляемыми часами (`TestClock`, `run_all_with_clock`) сценарии проверяют точные длительности. Самопроверка — эталонный мок в `crates/api-contract-tests/tests/reference_mock.rs` (только для тестов, не для UI).
- **`reqwest` без default features**: `json`, `stream`, `rustls`, `http2` — без системного OpenSSL. `sqlx` без default features: `runtime-tokio`, `sqlite` (bundled), `migrate`, `macros`, `chrono`, `uuid`, `json`.
- **Проверка шага 0** (`fmt`, `clippy --workspace --all-targets -D warnings`, `test --workspace`, `check --workspace`, wasm-check `focus-api-types`) — зелёная на всём workspace, включая `apps/`. `dx build` в этом окружении не запускался (нет `dx`); его прогоняет сессия B.
