# desktop — Focus Flow для Linux/macOS/Windows

Только `main.rs`: `DesktopPlatform` (уведомления через `notify-rust`), `HttpApi` (или `MockApi` в режиме демо) в контексте, окно 1180×780 (минимум 960×640).

```bash
dx serve            # разработка
dx build            # сборка
```

Переменные для демо и скриншотов:

- `FOCUS_MOCK_SCENARIO` — `idle` (по умолчанию), `work`, `rest`, `planning`, `picker`, `interrupt`, `done-choice`, `tasks`, `day`, `empty`;
- `FOCUS_MOCK_SPEED` — ускорение часов мока (`60` — минута за секунду, `0` — часы стоят).

## Сервис

По умолчанию клиент подключается к `focus-flow serve`:

- `FOCUS_ADDR` — адрес (по умолчанию `127.0.0.1:7878`);
- `FOCUS_TOKEN` — токен; без него читается файл `token` из `FOCUS_DATA_DIR` или из каталога данных сервиса (`~/.local/share/focus-flow/token` на Linux).

`FOCUS_MOCK=1` (или любой `FOCUS_MOCK_SCENARIO`) запускает клиент на `MockApi` без сервиса.
