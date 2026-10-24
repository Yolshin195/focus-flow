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

Кроме инструментов ядра, включённые плагины добавляют свои, например `jira_import(url)`. Каждый результат состоит из строки-сводки на русском и компактного JSON. Ошибки (например, `item_required`) возвращаются как ошибка инструмента и содержат подсказку, что делать дальше.
