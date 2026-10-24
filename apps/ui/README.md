# ui — общий интерфейс Focus Flow

Маршруты, экраны, компоненты и тема для всех платформ. Платформенных зависимостей нет.

```
ui/
├─ assets/
│  ├─ fonts/          # Inter и JetBrains Mono (woff2, OFL) — в комплекте, без сети
│  ├─ tokens.css      # дизайн-токены
│  └─ main.css        # стили экранов
└─ src/
   ├─ lib.rs          # App, enum Route
   ├─ platform.rs     # трейт Platform (уведомления, wake lock, адрес сервиса)
   ├─ state.rs        # общее состояние, опрос статуса раз в секунду, команды
   ├─ fonts.rs        # @font-face для шрифтов из assets/fonts
   ├─ fmt.rs          # форматирование времени
   ├─ api/mock.rs     # MockApi — FocusApi в памяти с живым таймером и демо-днём
   ├─ screens/        # focus.rs, tasks.rs, day.rs
   └─ components/     # ring, picker, dialog, norm_bar, icons
```

`cargo test -p ui` гоняет сценарии `focus-api-contract-tests` на `MockApi`.
