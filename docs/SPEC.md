# Focus Flow — спецификация v1

Полное ТЗ с обсуждением: https://claude.ai/artifact/RW1QVDDktmekdZB3Mz589Z
Этот файл — сжатая и окончательная версия решений для v1. При расхождении прав этот файл.

## Цель

Удерживать ритм 50 мин работы / 10 мин отдыха и показывать, набрана ли норма дня (по умолчанию 8 ч).

## Термины

- **День** — открывается вручную или автоматически при старте любой сессии; время начала = время старта. Закрывается вручную.
- **Сессия** — непрерывный отрезок одного вида: `work`, `rest`, `planning`. Одновременно активна максимум одна.
- **Задача** — узел дерева. Два уровня: задача (крупная, часто связана с Jira) → пункты (маленькие шаги). В работу берётся пункт; пункт без родителя допустим.
- **Норма** — сумма длительностей сессий `work` + `planning` за день. Отдых и потерянное время в норму не входят.

## Правила таймера

1. `work`: лимит 50 мин (настраивается), отсчёт вниз, обязательно привязана к одному пункту. Сменить пункт посреди сессии нельзя.
2. **Паузы нет.** Досрочно закрыть сессию без завершения пункта можно только через «Прервать» с подтверждением-предупреждением («Ты отвлекаешься. Эти 50 минут тебя ни для кого нет»). `end_reason = interrupted`.
3. «Готово» по пункту: сессия закрывается с фактической длительностью, пункт → `done`. Дальше выбор: `rest` или `planning` (планирование стартует сразу, без потерянного времени; после него — отдых).
4. Лимит 50 мин истёк: звук + системное уведомление, сессия закрывается (`end_reason = limit`), пункт остаётся открытым, сразу стартует `rest`.
5. `rest`: 10 мин (настраивается), всегда одинаково, независимо от длины работы. Пропустить нельзя.
6. `planning`: без лимита, отсчёт вверх, засчитывается в норму. Заканчивается кнопкой «Стоп» или стартом работы.
7. Время от конца отдыха до старта новой сессии — **потерянное**: на календаре серое, в норму не идёт.
8. `end_reason`: `task_done | limit | interrupted | stopped | switched`.
9. Источник истины — сервис: хранится `started_at`, остальное вычисляется. Перезапуск окна ничего не теряет.
10. Сессия через полночь относится ко дню старта.

## Архитектура

Один локальный сервис `focus-flow serve` (Axum, `127.0.0.1:7878`) — единственная точка входа и единственный владелец SQLite. Клиенты: desktop UI (Dioxus) и Claude (MCP). Позже тот же сервис переезжает на VDS (+ авторизация по токену), появится мобильный клиент.

Имена пакетов: в `crates/` и `bin/` — с префиксом `focus-` (`crates/core` → пакет `focus-core`; имя `core` занято стандартной библиотекой); в `apps/` — как из шаблона dx: `ui`, `desktop`, `web`, `mobile`.

```
crates/                        СЕРВИС
  api-types     контракт: DTO, ActiveSession, ApiError, трейт FocusApi, события SSE; собирается в wasm
  api-contract-tests  сценарии поведения против &dyn FocusApi — гоняются и на MockApi, и на сервисе
  core          домен, правила таймера, события, outbox; без Axum и SQL
  storage       SQLite (sqlx), миграции ядра, репозитории
  plugin-api    трейты Plugin, TaskSource; DomainEvent; PluginCtx
  plugin-jira   плагин Jira (feature "jira", включён по умолчанию)
  api-rest      Axum: REST + SSE
  api-mcp       rmcp, Streamable HTTP на /mcp (feature "mcp", включён по умолчанию)
  server        сборка: конфиг, реестр плагинов, таймер-воркер, запуск Axum
apps/                          КЛИЕНТЫ (Dioxus 0.7, общий UI)
  ui            экраны, компоненты, тема; трейты Platform и FocusApi; MockApi, HttpApi
  desktop       main.rs + Platform для desktop (первая версия)
  web           main.rs + Platform для браузера (заготовка)
  mobile        main.rs + Platform для iOS/Android (заготовка)
bin/focus-flow  CLI: `serve` | `ui` (ui поднимает serve, если не запущен)
```

Направление зависимостей (нарушать нельзя):

```
apps/ui ──▶ api-types ◀── core, api-rest, api-mcp
apps/{desktop,web,mobile} ──▶ apps/ui
server ──▶ core, storage, plugin-api, plugin-jira, api-rest, api-mcp
bin/focus-flow ──▶ server, apps/desktop
```

`apps/*` никогда не зависят от `crates/*`, кроме `api-types`. `core` не зависит от `storage`, Axum и HTTP — только от трейтов (порты), реализации подставляет `server`.

Безопасность localhost: bind только 127.0.0.1; токен в заголовке `Authorization: Bearer` (генерируется при первом запуске, хранится в системном хранилище ключей — крейт `keyring`; для разработки допускается файл в data dir); проверка `Host`/`Origin` против DNS rebinding.

## Плагины

```rust
#[async_trait]
pub trait Plugin: Send + Sync {
    fn id(&self) -> &'static str;
    fn migrations(&self) -> sqlx::migrate::Migrator;          // свои таблицы, префикс <id>_
    async fn on_event(&self, ev: &DomainEvent, ctx: &PluginCtx) -> anyhow::Result<()>;
    fn routes(&self) -> Option<axum::Router<AppState>> { None }
    fn mcp_tools(&self) -> Vec<McpToolDef> { vec![] }
}

#[async_trait]
pub trait TaskSource: Plugin {
    fn matches(&self, url: &str) -> bool;
    async fn import(&self, url: &str, ctx: &PluginCtx) -> anyhow::Result<ImportedTask>;
}
```

- Ядро публикует `DomainEvent`: `ItemCreated`, `ItemCompleted`, `SessionClosed`, `TaskImported`.
- Доставка — через таблицу `outbox` и фоновый воркер с повторами (экспоненциальная задержка). Без сети ничего не теряется; ошибки видны в UI (`/api/plugins/status`).
- Подключение: cargo features + `enabled` в конфиге. Без динамической загрузки.
- REST и MCP — входы, не плагины. MCP отдаёт инструменты ядра + `mcp_tools()` включённых плагинов.

## Плагин Jira

- Задача Jira ↔ наша задача верхнего уровня. Пункты = строки **Smart Checklist** (Railsware) этой задачи.
- **Импорт по ссылке/ключу:** создаёт задачу (название, URL) и сразу все строки чек-листа как пункты. Статусы: `+` → done, `x` → cancelled, остальное → planned. Повторный импорт не дублирует, а догружает новые строки.
- **Новый пункт у нас** → добавляется строкой `- текст` в Smart Checklist.
- **Пункт готов** → строка меняется на `+ текст`.
- **Worklog** — на саму задачу Jira (не на пункт): `started` = реальное начало, `timeSpentSeconds` = длительность, `comment` = текст пункта. По умолчанию списываются сессии `work` с любым `end_reason` (включая `interrupted`); `planning` — нет. Оба правила — в настройках плагина.
- Smart Checklist хранится в кастомном поле задачи «Checklists» (текст, строка на пункт, маркеры `-` todo, `+` done, `~` in progress, `x` skipped). Писать — только целиком через `PUT /rest/api/2/issue/{key}` с этим полем. ID поля ищется при настройке через `GET /rest/api/2/field` по имени (можно задать вручную). У строк нет ID → сопоставление по тексту; перед каждой записью — перечитать поле и применить изменение к свежему тексту.
- Авторизация — **только API-токен**. Два режима: Data Center/Server — `Authorization: Bearer <PAT>`; Cloud — Basic `email:token`. Режим и base URL — в настройках. Какой у пользователя — пока не подтверждено, поддержать оба.
- Тесты — на `wiremock`, без реальной Jira.
- **Очередь отправки.** Новые строки, отметки «готово» и worklog не уходят в Jira сразу, а копятся в очереди плагина: её видно на экране «Настройки», лишнее можно убрать, остальное — отправить кнопкой «Отправить». Настройка «Отправлять автоматически» (`auto_send`, по умолчанию выключена) отправляет очередь сразу. Маршруты: `GET /api/plugins/jira/queue`, `POST …/queue/send`, `DELETE …/queue/{id}`.
- Таблицы плагина: `jira_links(task_id, issue_key, url)`, `jira_items(item_id, issue_key, line_text)`, `jira_worklogs(session_id, issue_key, worklog_id, status)`, `jira_queue` (очередь отправки).

## REST API (v1)

Все ответы — DTO из `api-types`. Время — RFC3339 UTC; длительности — секунды.

| Метод | Путь | Что делает |
|---|---|---|
| GET | `/api/status` | `StatusDto`: день, активная сессия, `ends_at`, отработано/норма, `server_time` |
| POST | `/api/day/open` · `/api/day/close` | открыть / закрыть день |
| GET | `/api/days/{date}` | `DayDto`: сессии дня + итоги (для календаря) |
| GET | `/api/tasks` | дерево задач (`TaskDto` с `children`) |
| POST | `/api/tasks` | создать задачу или пункт (`parent_id` опционален) |
| PATCH | `/api/tasks/{id}` | правка: название, заметки, статус, `planned_for` |
| POST | `/api/sessions/work` | `{item_id}` — старт работы |
| POST | `/api/sessions/planning` | старт планирования |
| POST | `/api/sessions/rest` | старт отдыха (после «Готово» или планирования) |
| POST | `/api/sessions/active/done` | «Готово»: закрыть сессию + пункт done; тело `{next: "rest" \| "planning"}` |
| POST | `/api/sessions/active/interrupt` | «Прервать» |
| POST | `/api/sessions/active/stop` | стоп планирования |
| POST | `/api/import` | `{url}` — импорт задачи через подходящий `TaskSource` |
| GET | `/api/settings` · PUT `/api/settings` | длины, норма, настройки плагинов |
| GET | `/api/plugins/status` | состояние плагинов и очереди outbox |
| GET | `/api/events` | SSE: `status_changed`, `session_ended`, `task_changed`, `plugin_error` |

## MCP-инструменты (v1)

`get_status`, `open_day`, `close_day`, `list_tasks`, `add_task`, `update_task`, `start_work(item_id)`, `finish_item(next)`, `interrupt`, `start_planning`, `stop`, `start_rest`, `get_day_report(date)`; от плагина Jira — `jira_import(url)`.

`get_status` всегда возвращает `ends_at` и `events` (что случилось с прошлого вызова: закончилась сессия, ошибки плагинов).

## Данные (ядро)

- `work_days(id, date, opened_at, closed_at, target_minutes)`
- `tasks(id, parent_id, title, notes, status[planned|in_progress|done|cancelled], planned_for, estimate_intervals, sort_order, created_at, done_at)`
- `sessions(id, day_id, kind, task_id, started_at, ended_at, limit_sec, end_reason)`
- `settings(key, value)`; `outbox(id, event_json, plugin_id, attempts, next_attempt_at, last_error, done_at)`

## UI (desktop, v1)

Платформенные различия — через трейт `Platform` в `apps/ui` (уведомления, wake lock, базовый URL сервиса), каждая платформа кладёт свою реализацию в контекст в своём `main.rs`. Стилевые различия — CSS-класс `platform-desktop|web|mobile` на корне.

Экраны: **Фокус** (кольцо таймера, пункт, одна главная кнопка по контексту, прогресс «5:20 из 8:00», начало дня), **Задачи** (дерево, быстрый ввод, вставка ссылки Jira, «Начать планирование»), **День** (вертикальная шкала: работа — синий, отдых — зелёный, планирование — жёлтый, потерянное — серый; клик → детали сессии). Тёмная тема по умолчанию, крупный таймер, стандартные компоненты Dioxus (dioxuslabs.com/components).

## Вне v1

Мобильный клиент и VDS, геймификация, ручные правки сессий задним числом, офлайн-синхронизация между устройствами.
