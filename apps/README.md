# Клиенты Focus Flow (apps/)

Клиенты на Dioxus 0.7: общий крейт `ui` и по крейту на каждую платформу. Сейчас разрабатывается только desktop; `web` и `mobile` — заготовки.

```
apps/
├─ README.md
├─ CLAUDE.md          # правила для Claude + официальные инструкции Dioxus (.llm/)
├─ ui/                # общий UI для всех платформ
├─ desktop/           # точка входа desktop (текущая версия)
├─ web/               # точка входа web (заготовка)
└─ mobile/            # точка входа iOS/Android (заготовка)
```

## Общий крейт `ui`

Здесь живёт весь интерфейс: маршруты, экраны, компоненты, тема, работа с сервисом.

```
ui/
├─ assets/
│  ├─ tokens.css      # дизайн-токены: цвета, отступы, радиусы, шрифты
│  └─ main.css
├─ src/
│  ├─ lib.rs          # pub fn App, enum Route
│  ├─ platform.rs     # трейт Platform: уведомления, wake lock, адрес сервиса
│  ├─ api/
│  │  ├─ mod.rs       # трейт FocusApi (из crates/api-types)
│  │  ├─ mock.rs      # MockApi — данные и живой таймер без сервиса
│  │  ├─ http.rs      # HttpApi — REST + SSE к focus-flow serve
│  │  └─ jira.rs      # JiraSettingsApi — настройки плагина Jira (HttpApi, MockJira)
│  ├─ screens/        # focus.rs, tasks.rs, day.rs, settings.rs
│  └─ components/     # timer_ring, task_tree, day_timeline, …
```

Правила:

- `ui` не знает про платформу. Всё, что отличается между платформами, идёт через трейт `Platform` из контекста.
- Никаких платформенных зависимостей в `ui` (`notify-rust`, `web-sys` и т.п.) — они в крейтах платформ.
- Данные — только через `FocusApi`; из компонентов не ходим в HTTP напрямую.
- Зависимости `ui`: `dioxus`, `crates/api-types` и кроссплатформенное для `HttpApi` (`reqwest` без TLS, `serde`, `futures-*`) — без платформенных крейтов.

## Крейты платформ

В каждом — только то, что специфично для платформы:

```
desktop/
├─ assets/            # ассеты только для desktop (иконка и т.п.)
├─ src/
│  ├─ lib.rs          # реализация Platform для desktop, launch() — запуск ui::App
│  └─ main.rs         # вход для dx serve / dx build
├─ Cargo.toml         # dioxus с feature "desktop" + платформенные зависимости
└─ Dioxus.toml
```

Экраны по умолчанию общие. Если платформе нужен свой вариант экрана, он появляется в её крейте и подменяет общий — это исключение, а не правило.

## Запуск

```bash
cd apps/desktop
dx serve
```

Hot reload: правки в `apps/ui` и `apps/desktop` применяются без перезапуска окна.

Сборка без запуска:

```bash
cd apps/desktop
dx build
```

Сервис должен быть запущен (`cargo run -p focus-flow -- serve`), если клиент работает через `HttpApi`. С `MockApi` сервис не нужен. Окно и сервис одной командой — `cargo run -p focus-flow -- ui` из корня.