# focus-flow

Трекер фокуса 50/10: работа 50 минут над одним пунктом, затем отдых 10 минут, без паузы. Показывает, набрана ли норма дня. Сервис локальный (`focus-flow serve`, Axum + SQLite на `127.0.0.1:7878`), клиенты — desktop UI на Dioxus и Claude через MCP.

Спецификация — [`docs/SPEC.md`](docs/SPEC.md), план — [`docs/PLAN-backend.md`](docs/PLAN-backend.md), решения — [`docs/DECISIONS.md`](docs/DECISIONS.md).

## Запуск сервиса

```sh
cargo run -p focus-flow -- serve      # REST + SSE + MCP на 127.0.0.1:7878
cargo run -p focus-flow -- token      # показать токен доступа
```

Данные (`focus.db`, `token`, `secrets/`) лежат в системном каталоге данных: на Linux это `~/.local/share/focus-flow`, на macOS — `~/Library/Application Support/dev.FocusFlow.focus-flow`. Другой каталог задаётся через `--data-dir` или `FOCUS_DATA_DIR`.

## Подключение к Claude

MCP работает на `http://127.0.0.1:7878/mcp` (Streamable HTTP). Доступ защищён тем же токеном, что и REST: заголовок `Authorization: Bearer <токен>`. Сначала запустите `focus-flow serve`.

### Claude Code

```sh
claude mcp add --transport http focus-flow http://127.0.0.1:7878/mcp --header "Authorization: Bearer $(focus-flow token)"
```

Команда `focus-flow` должна быть в `PATH`, например после `cargo install --path bin/focus-flow`. Можно и без установки: `$(cargo run -q -p focus-flow -- token)`. Проверить подключение — `claude mcp list` или `/mcp` внутри сессии. Если токен поменялся (удалили файл `token`), удалите сервер командой `claude mcp remove focus-flow` и добавьте его заново.

### Claude Desktop

Claude Desktop подключает локальные серверы через stdio. HTTP-сервер с заголовком подключается через мост [`mcp-remote`](https://www.npmjs.com/package/mcp-remote) (нужен Node.js). Добавьте в `claude_desktop_config.json` (Settings → Developer → Edit Config):

```json
{
  "mcpServers": {
    "focus-flow": {
      "command": "npx",
      "args": [
        "-y", "mcp-remote", "http://127.0.0.1:7878/mcp",
        "--header", "Authorization:${FOCUS_AUTH}"
      ],
      "env": { "FOCUS_AUTH": "Bearer <вывод focus-flow token>" }
    }
  }
}
```

Пробела внутри аргумента `--header` нет специально: часть клиентов неправильно передаёт аргументы с пробелами, поэтому `Bearer …` лежит в переменной окружения. После правки перезапустите Claude Desktop.

### Инструменты

| Инструмент | Что делает |
|---|---|
| `get_status` | текущая сессия, `ends_at`, `remaining_sec`, отработано и норма; `events` — что случилось с прошлого вызова (сессия закончилась, ошибки плагинов) |
| `open_day` · `close_day` | открыть или закрыть сегодняшний день |
| `list_tasks` | дерево задач (по умолчанию только открытые) |
| `add_task` | задача, или пункт, если передан `parent_id`; поле `notes` необязательно |
| `update_task` | правка названия, заметок, статуса, `planned_for` |
| `start_work(item_id)` | начать 50 минут работы над пунктом |
| `finish_item(next)` | «Готово»: пункт выполнен, дальше `rest` или `planning` |
| `interrupt` | «Прервать» работу (отвлёкся) |
| `start_planning` · `stop` · `start_rest` | планирование, его остановка, отдых |
| `get_day_report(date?)` | итоги дня: работа, планирование, отдых, потерянное время |

Кроме инструментов ядра, включённые плагины добавляют свои: плагин Jira — `jira_import(url)` и `jira_status()`. Каждый результат состоит из строки-сводки на русском и компактного JSON. Ошибки (например, `item_required`) возвращаются как ошибка инструмента и содержат подсказку, что делать дальше.

## Настройка Jira

Плагин Jira собран в `focus-flow` по умолчанию и включён, но пока он не настроен, ничего не делает: события пропускаются без сети, а в `GET /api/plugins/status` у плагина `jira` написано «не настроен: …». Экран настроек в UI появится позже; сейчас плагин настраивается через REST (или спросите у Claude `jira_status`).

Что делает настроенный плагин:

- `POST /api/import` (или `jira_import`) со ссылкой `https://<ваша jira>/browse/KEY-123` или ключом `KEY-123` создаёт задачу «KEY-123 · название» и пункты из строк Smart Checklist (`-` — запланирован, `+` — готов, `~` — в работе, `x` — отменён; заголовки `#` пропускаются). Повторный импорт догружает только новые строки.
- Новый пункт в такой задаче → строка `- текст` в чек-листе; пункт готов → строка становится `+ текст`. Перед каждой записью поле перечитывается, поэтому правки, сделанные в Jira, не теряются.
- Закрытая сессия работы → worklog на задачу Jira: начало, длительность (не меньше минуты), комментарий — текст пункта. По умолчанию списывается любая работа, включая прерванную; планирование — нет.

### Токен

- **Jira Data Center / Server** — персональный токен доступа (PAT): аватар → **Profile** → **Personal Access Tokens** → **Create token**. Режим `data_center`, запросы идут с `Authorization: Bearer <PAT>`.
- **Jira Cloud** — API-токен: <https://id.atlassian.com/manage-profile/security/api-tokens> → **Create API token**. Режим `cloud`, нужен ещё `email` учётной записи; запросы идут с Basic `email:токен`.

Токен хранится в `secrets/jira/token` в каталоге данных (права `0600`). Он не попадает в настройки, ответы API и логи.

### Настройки

```sh
TOKEN=$(focus-flow token)

# Data Center
curl -s -X PUT http://127.0.0.1:7878/api/plugins/jira/settings \
  -H "Authorization: Bearer $TOKEN" -H "Content-Type: application/json" \
  -d '{"base_url": "https://jira.example.com", "mode": "data_center", "token": "<PAT>"}'

# Cloud
curl -s -X PUT http://127.0.0.1:7878/api/plugins/jira/settings \
  -H "Authorization: Bearer $TOKEN" -H "Content-Type: application/json" \
  -d '{"base_url": "https://example.atlassian.net", "mode": "cloud", "email": "me@example.com", "token": "<API-токен>"}'

# Проверить подключение: в ответе — имя пользователя Jira или ошибка
curl -s -X POST http://127.0.0.1:7878/api/plugins/jira/test -H "Authorization: Bearer $TOKEN"

# Посмотреть настройки (вместо токена — has_token)
curl -s http://127.0.0.1:7878/api/plugins/jira/settings -H "Authorization: Bearer $TOKEN"
```

`PUT` меняет только переданные поля; `null` очищает поле, `"token": ""` удаляет токен.

| Поле | Значение |
|---|---|
| `base_url` | адрес Jira, например `https://jira.example.com` |
| `mode` | `data_center` (по умолчанию) или `cloud` |
| `email` | email учётной записи — только для `cloud` |
| `token` | PAT или API-токен; только запись |
| `checklist_field_id` | id поля Smart Checklist, например `customfield_10100`. Если не задан, плагин сам ищет его через `GET /rest/api/2/field` по имени «Smart Checklist» (или содержащему «Checklist») |
| `worklog` | что списывать: `{"work": true, "interrupted": true, "planning": false}` |
| `enabled` | `false` выключает плагин целиком |

Если запись в Jira не прошла (нет сети, 5xx, неверный токен), событие остаётся в очереди и повторяется с нарастающей задержкой; текст ошибки виден в `GET /api/plugins/status`.
