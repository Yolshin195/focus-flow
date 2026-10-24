# desktop — Focus Flow для Linux/macOS/Windows

Только `main.rs`: `DesktopPlatform` (уведомления через `notify-rust`), `MockApi` в контексте, окно 1180×780 (минимум 960×640).

```bash
dx serve            # разработка
dx build            # сборка
```

Переменные для демо и скриншотов:

- `FOCUS_MOCK_SCENARIO` — `idle` (по умолчанию), `work`, `rest`, `planning`, `picker`, `interrupt`, `done-choice`, `tasks`, `day`, `empty`;
- `FOCUS_MOCK_SPEED` — ускорение часов мока (`60` — минута за секунду, `0` — часы стоят).
