# desktop — Focus Flow для Linux/macOS/Windows

`lib.rs`: `DesktopPlatform` (уведомления через `notify-rust`), `HttpApi` (или `MockApi` в режиме демо) в контексте, окно 1180×780 (минимум 960×640); вход — `desktop::launch(Options, on_exit)`. Его вызывают `main.rs` (для `dx serve`/`dx build`) и `focus-flow ui` (окно + сервис в одном процессе).

`tray.rs` — трей / строка меню: значок фазы, время рядом с ним, меню команд по фазе (см. раздел «Трей / строка меню» в корневом README). Пока трей есть, закрытие окна его только прячет. Значки — `assets/tray/` (SVG-исходники в `src/`, PNG пересобирает `assets/tray/render.sh`).

```bash
dx serve            # разработка
dx build            # сборка
```

Переменные для демо и скриншотов:

- `FOCUS_MOCK_SCENARIO` — `idle` (по умолчанию), `work`, `rest`, `planning`, `picker`, `interrupt`, `done-choice`, `tasks`, `day`, `settings`, `empty`;
- `FOCUS_MOCK_JIRA=off` — Jira в моке не настроена;
- `FOCUS_MOCK_SPEED` — ускорение часов мока (`60` — минута за секунду, `0` — часы стоят).

## Сервис

По умолчанию клиент подключается к `focus-flow serve`:

- `FOCUS_ADDR` — адрес (по умолчанию `127.0.0.1:7878`);
- `FOCUS_TOKEN` — токен; без него читается файл `token` из `FOCUS_DATA_DIR` или из каталога данных сервиса (`~/.local/share/focus-flow/token` на Linux).

`FOCUS_MOCK=1` (или любой `FOCUS_MOCK_SCENARIO`) запускает клиент на `MockApi` без сервиса.
