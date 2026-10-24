# UI (apps/)

Официальные инструкции Dioxus 0.7 — не редактировать, обновлять только заменой файла:
@.llm/dioxus-0.7.md

## Уточнения для этого проекта (приоритет над файлом выше)

- Fullstack не используем: сервер — отдельный Axum-сервис. Features `server`, `fullstack` и `#[post]`/`#[get]` не нужны; данные — только через трейт `FocusApi`.
- Платформа включается feature в крейте платформы (`apps/desktop`: `desktop = ["dioxus/desktop"]`), а не в `apps/ui`.
- `apps/ui` не знает про платформу: всё платформенное — через трейт `Platform` из контекста.
- Стили — `document::Stylesheet` + `asset!()`; дизайн-токены в `apps/ui/assets/tokens.css`.