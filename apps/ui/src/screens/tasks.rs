//! Экран «Задачи»: дерево задач → пункты, быстрый ввод, импорт из Jira.

use std::collections::HashSet;

use dioxus::prelude::*;
use focus_api_types::chrono::Utc;
use focus_api_types::{ActiveSession, ApiError, NewTask, TaskDto, TaskId, TaskStatus};

use crate::api::jira_key;
use crate::components::{Icon, TextArea};
use crate::fmt;
use crate::state::{run, use_api, use_app, Command};
use crate::Route;

fn is_done(t: &TaskDto) -> bool {
    matches!(t.status, TaskStatus::Done | TaskStatus::Cancelled)
}

/// Что получится из текста в поле ввода.
#[derive(Debug, Clone, PartialEq)]
enum Draft {
    Empty,
    /// Одна строка — пункт без задачи.
    Text(String),
    /// Ссылка или ключ Jira: (как ввели, ключ).
    Jira(String, String),
    /// Несколько строк: задача и её пункты.
    List(String, Vec<String>),
}

/// Строка — ссылка Jira, если это URL с ключом задачи или сам ключ: ключ
/// внутри обычного названия («Починить PROJ-12») импорт не запускает.
fn parse_draft(text: &str) -> Draft {
    let lines: Vec<&str> = text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    match lines.as_slice() {
        [] => Draft::Empty,
        [one] => match jira_key(one) {
            Some(key) if one.contains("://") || *one == key => Draft::Jira((*one).to_string(), key),
            _ => Draft::Text((*one).to_string()),
        },
        [first, rest @ ..] => Draft::List(
            (*first).to_string(),
            rest.iter().map(|l| (*l).to_string()).collect(),
        ),
    }
}

/// Строки текста — названия пунктов (пустые пропускаются).
fn item_lines(text: &str) -> Vec<String> {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(String::from)
        .collect()
}

/// Команда «добавить пункты к `parent`» из текста: строка — пункт.
fn add_items(parent: TaskId, text: &str) -> Option<Command> {
    let mut lines = item_lines(text);
    match lines.len() {
        0 => None,
        1 => lines
            .pop()
            .map(|t| Command::Create(NewTask::child(parent, t))),
        _ => Some(Command::CreateItems(parent, lines)),
    }
}

fn shift(e: &KeyboardEvent) -> bool {
    e.modifiers().contains(Modifiers::SHIFT)
}

/// Экран «Задачи».
#[component]
pub fn Tasks() -> Element {
    let api = use_api();
    let app = use_app();
    let mut text = use_signal(String::new);
    // Пересоздать поле (очистить) после «Создать» / «Импорт».
    let mut text_reset = use_signal(|| 0u32);
    let collapsed = use_signal(HashSet::<TaskId>::new);

    let tree = app.tasks.read().clone();
    // Пункты без родителя — одной группой в конце.
    let loose: Vec<TaskDto> = tree.iter().filter(|t| t.is_item()).cloned().collect();
    let open_items: usize = tree
        .iter()
        .map(|t| {
            if t.is_item() {
                usize::from(!is_done(t))
            } else {
                t.children.iter().filter(|c| !is_done(c)).count()
            }
        })
        .sum();
    let idle = app
        .status
        .read()
        .as_ref()
        .is_some_and(|s| s.active.is_none());
    let planning = matches!(
        app.status.read().as_ref().and_then(|s| s.active.as_ref()),
        Some(ActiveSession::Planning { .. })
    );
    let busy = (app.busy)();
    let nav = use_navigator();

    // Подключена ли Jira: без неё импорт не сработает — скажем об этом, когда
    // вставят ссылку, и дадим ссылку на настройки. Ошибка запроса — считаем,
    // что неизвестно.
    let api_jira = api.clone();
    let jira = use_resource(move || {
        let api = api_jira.clone();
        async move { api.jira().jira_settings().await }
    });
    let jira_note: Option<&str> = match &*jira.read() {
        Some(Ok(s)) if !s.enabled => Some("Плагин Jira выключен."),
        Some(Ok(s)) if !s.configured => Some("Jira не подключена."),
        Some(Err(ApiError::Transport(m))) if m.contains("404") => {
            Some("Плагин Jira не собран в этот сервис.")
        }
        _ => None,
    };
    let jira_ready = jira_note.is_none();

    let draft = parse_draft(&text.read());
    let can_submit = match &draft {
        Draft::Empty => false,
        Draft::Jira(..) => jira_ready && !busy,
        _ => !busy,
    };
    let api_submit = api.clone();
    let submit = move || {
        let cmd = match parse_draft(&text.peek()) {
            Draft::Empty => return,
            Draft::Jira(url, _) if jira_ready => Command::Import(url),
            Draft::Jira(..) => return,
            Draft::Text(t) => Command::Create(NewTask::new(t)),
            Draft::List(t, items) => Command::CreateWithItems(NewTask::new(t), items),
        };
        run(api_submit.clone(), app, cmd, move || {
            text.set(String::new());
            text_reset += 1;
        });
    };
    let submit_enter = submit.clone();
    let submit_click = submit;
    let api_plan = api.clone();

    rsx! {
        div { class: "page tasks",
            header { class: "page-head",
                div {
                    h1 { class: "page-title", "Задачи" }
                    p { class: "page-sub",
                        "{open_items} {fmt::plural(open_items, \"открытый пункт\", \"открытых пункта\", \"открытых пунктов\")}"
                    }
                }
                button {
                    class: "btn btn-secondary btn-planning-outline",
                    disabled: busy || !idle,
                    title: if !idle && !planning { "Сейчас идёт сессия" } else { "" },
                    onclick: move |_| {
                        let nav = nav;
                        run(api_plan.clone(), app, Command::StartPlanning, move || {
                            nav.push(Route::Focus {});
                        });
                    },
                    if planning { "Планирование идёт" } else { "Начать планирование" }
                }
            }

            div { class: "composer",
                label { class: "field field-new",
                    {Icon::Plus.render(18)}
                    TextArea {
                        reset: text_reset(),
                        refocus: true,
                        placeholder: "Задача, пункт или ссылка Jira",
                        label: "Новая задача, пункт или ссылка на задачу Jira",
                        spellcheck: !matches!(draft, Draft::Jira(..)),
                        on_input: move |v| text.set(v),
                        on_keydown: move |e: KeyboardEvent| {
                            // Enter — создать, Shift+Enter — новая строка (будущий пункт).
                            if e.key() == Key::Enter && !shift(&e) {
                                e.prevent_default();
                                submit_enter();
                            }
                        },
                    }
                    match &draft {
                        Draft::Empty => rsx! {},
                        Draft::Jira(_, key) => rsx! {
                            button {
                                class: "btn btn-small btn-accent",
                                disabled: !can_submit,
                                onclick: move |_| submit_click(),
                                "Импорт {key}"
                            }
                        },
                        Draft::List(_, items) => rsx! {
                            button {
                                class: "btn btn-small",
                                disabled: !can_submit,
                                onclick: move |_| submit_click(),
                                "Создать + {items.len()}"
                            }
                        },
                        Draft::Text(_) => rsx! {
                            button {
                                class: "btn btn-small",
                                disabled: !can_submit,
                                onclick: move |_| submit_click(),
                                "Создать"
                            }
                        },
                    }
                }
                match &draft {
                    Draft::Jira(_, key) => rsx! {
                        p { class: "composer-hint", role: "note",
                            if let Some(note) = jira_note {
                                "{note} "
                                Link { to: Route::Settings {}, "Подключить в настройках" }
                            } else {
                                "Задача Jira "
                                b { "{key}" }
                                ": подтянем название и чек-лист, строки станут пунктами."
                            }
                        }
                    },
                    Draft::List(title, items) => rsx! {
                        p { class: "composer-hint",
                            "Задача «"
                            b { "{title}" }
                            "» и {items.len()} {fmt::plural(items.len(), \"пункт\", \"пункта\", \"пунктов\")}. Первая строка — задача, остальные — пункты."
                        }
                    },
                    _ => rsx! {},
                }
            }

            if tree.is_empty() {
                div { class: "empty",
                    h2 { class: "empty-title", "Задач пока нет" }
                    p { class: "empty-text",
                        "Напиши задачу и нажми Enter или вставь ссылку на задачу Jira — пункты чек-листа станут шагами. Несколько строк (Shift+Enter) — задача и её пункты."
                    }
                }
            }

            div { class: "tree",
                for task in tree.iter().filter(|t| !t.is_item()).cloned() {
                    TaskNode { key: "{task.id}", task, collapsed }
                }
                if !loose.is_empty() {
                    div { class: "task task-loose",
                        div { class: "task-loose-title", "Без задачи" }
                        div { class: "items",
                            for item in loose.iter().filter(|t| !is_done(t)).cloned() {
                                ItemRow { key: "{item.id}", item, loose: true }
                            }
                            DoneFold { items: loose.iter().filter(|t| is_done(t)).cloned().collect::<Vec<_>>(), loose: true }
                        }
                    }
                }
            }
        }
    }
}

/// Задача: кольцо прогресса, название (нажатие сворачивает), открытые пункты,
/// «Добавить пункт» последней строкой, готовые — свёрнуты.
#[component]
fn TaskNode(task: TaskDto, collapsed: Signal<HashSet<TaskId>>) -> Element {
    let api = use_api();
    let app = use_app();
    let mut collapsed = collapsed;
    let mut adding = use_signal(|| false);
    let mut draft = use_signal(String::new);
    let mut draft_reset = use_signal(|| 0u32);
    let id = task.id;
    let is_collapsed = collapsed.read().contains(&id);
    let key = task.external_url.as_deref().and_then(jira_key);
    let done = is_done(&task);
    let total = task.children.len();
    let closed = task.children.iter().filter(|c| is_done(c)).count();
    let open: Vec<TaskDto> = task
        .children
        .iter()
        .filter(|c| !is_done(c))
        .cloned()
        .collect();
    let finished: Vec<TaskDto> = task
        .children
        .iter()
        .filter(|c| is_done(c))
        .cloned()
        .collect();
    let left = open.len();
    let intervals: u32 = open.iter().filter_map(|c| c.estimate_intervals).sum();
    let summary = if done {
        "задача закрыта".to_string()
    } else if left == 0 {
        "все пункты готовы".to_string()
    } else {
        let mut s = format!(
            "осталось {left} {}",
            fmt::plural(left, "пункт", "пункта", "пунктов")
        );
        if intervals > 0 {
            s.push_str(&format!(" · ≈ {intervals} × 50 мин"));
        }
        s
    };

    let api_add = api.clone();
    let mut add = move || {
        let Some(cmd) = add_items(id, &draft.peek()) else {
            adding.set(false);
            return;
        };
        run(api_add.clone(), app, cmd, move || {
            draft.set(String::new());
            draft_reset += 1;
        });
    };

    rsx! {
        div { class: if done { "task done" } else { "task" },
            div { class: "task-head",
                span { class: "task-ring", title: "Готово {closed} из {total}",
                    ProgressRing { done: closed, total }
                }
                div { class: "task-main",
                    button {
                        class: "task-toggle",
                        "aria-expanded": "{!is_collapsed}",
                        title: if is_collapsed { "Развернуть" } else { "Свернуть" },
                        onclick: move |_| {
                            let mut set = collapsed.write();
                            if !set.remove(&id) {
                                set.insert(id);
                            }
                        },
                        span { class: "task-title", "{task.title}" }
                        span { class: if is_collapsed { "task-caret collapsed" } else { "task-caret" },
                            {Icon::Chevron.render(14)}
                        }
                    }
                    div { class: "task-sub",
                        if let (Some(k), Some(url)) = (&key, &task.external_url) {
                            a { class: "chip chip-jira chip-link", href: "{url}", target: "_blank", title: "Открыть в Jira",
                                "{k}"
                                {Icon::External.render(12)}
                            }
                        }
                        span {
                            span { class: "num", "{closed} из {total}" }
                            " · {summary}"
                        }
                    }
                }
                TaskMenu { id, done, url: task.external_url.clone() }
            }
            if !is_collapsed {
                div { class: "items",
                    for item in open.iter().cloned() {
                        ItemRow { key: "{item.id}", item, loose: false }
                    }
                    if adding() {
                        div { class: "item-add",
                            TextArea {
                                reset: draft_reset(),
                                refocus: true,
                                class: "inline-input",
                                placeholder: "Новый пункт — Enter; несколько строк — несколько пунктов",
                                label: "Новый пункт",
                                autofocus: true,
                                on_input: move |v| draft.set(v),
                                on_keydown: move |e: KeyboardEvent| match e.key() {
                                    Key::Enter if !shift(&e) => {
                                        e.prevent_default();
                                        add();
                                    }
                                    Key::Escape => {
                                        draft.set(String::new());
                                        adding.set(false);
                                    }
                                    _ => {}
                                },
                                on_blur: move |_| {
                                    if draft.peek().trim().is_empty() {
                                        adding.set(false);
                                    }
                                },
                            }
                        }
                    } else if !done {
                        button { class: "item-add-btn", onclick: move |_| adding.set(true),
                            span { class: "item-add-icon", {Icon::Plus.render(16)} }
                            "Добавить пункт"
                        }
                    }
                    DoneFold { items: finished, loose: false }
                }
            }
        }
    }
}

/// Кольцо: доля готовых пунктов; всё готово — зелёное с галочкой.
#[component]
fn ProgressRing(done: usize, total: usize) -> Element {
    const R: f64 = 8.0;
    let len = 2.0 * std::f64::consts::PI * R;
    let full = total > 0 && done >= total;
    let arc = if total == 0 {
        0.0
    } else {
        len * done as f64 / total as f64
    };
    rsx! {
        svg {
            class: if full { "ring-mini full" } else { "ring-mini" },
            width: "22",
            height: "22",
            view_box: "0 0 22 22",
            "aria-hidden": "true",
            circle { class: "ring-mini-track", cx: "11", cy: "11", r: "{R}" }
            if done > 0 {
                circle {
                    class: "ring-mini-arc",
                    cx: "11",
                    cy: "11",
                    r: "{R}",
                    stroke_dasharray: "{arc:.2} {len:.2}",
                    transform: "rotate(-90 11 11)",
                }
            }
            if full {
                path { class: "ring-mini-check", d: "M7.6 11.2l2.2 2.2 4.4-4.6" }
            }
        }
    }
}

/// Меню «⋯» задачи: закрыть или вернуть задачу, открыть в Jira.
/// Закрыть задачу одним случайным касанием по кольцу больше нельзя.
#[component]
fn TaskMenu(id: TaskId, done: bool, url: Option<String>) -> Element {
    let api = use_api();
    let app = use_app();
    let mut open = use_signal(|| false);
    rsx! {
        div { class: "task-menu",
            button {
                class: "icon-btn task-more",
                "aria-label": "Действия с задачей",
                "aria-haspopup": "menu",
                "aria-expanded": "{open()}",
                onclick: move |_| open.toggle(),
                {Icon::More.render(18)}
            }
            if open() {
                button {
                    class: "menu-backdrop",
                    tabindex: "-1",
                    "aria-hidden": "true",
                    onclick: move |_| open.set(false),
                }
                div {
                    class: "menu",
                    role: "menu",
                    onkeydown: move |e: KeyboardEvent| {
                        if e.key() == Key::Escape {
                            open.set(false);
                        }
                    },
                    button {
                        class: "menu-item",
                        role: "menuitem",
                        // Фокус в меню: Esc закрывает, Tab идёт по пунктам.
                        onmounted: move |e| async move {
                            let _ = e.set_focus(true).await;
                        },
                        onclick: move |_| {
                            open.set(false);
                            run(api.clone(), app, Command::SetDone(id, !done), || {});
                        },
                        {Icon::Check.render(16)}
                        if done { "Вернуть задачу" } else { "Закрыть задачу" }
                    }
                    if let Some(u) = url {
                        a {
                            class: "menu-item",
                            role: "menuitem",
                            href: "{u}",
                            target: "_blank",
                            onclick: move |_| open.set(false),
                            {Icon::External.render(16)}
                            "Открыть в Jira"
                        }
                    }
                }
            }
        }
    }
}

/// Готовые и отменённые пункты — свёрнуты в одну строку «Готово · N».
#[component]
fn DoneFold(items: Vec<TaskDto>, loose: bool) -> Element {
    let mut shown = use_signal(|| false);
    if items.is_empty() {
        return rsx! {};
    }
    let n = items.len();
    rsx! {
        button {
            class: "fold",
            "aria-expanded": "{shown()}",
            onclick: move |_| shown.toggle(),
            span { class: if shown() { "fold-caret" } else { "fold-caret collapsed" }, {Icon::Chevron.render(14)} }
            span { "Готово · " span { class: "num", "{n}" } }
        }
        if shown() {
            for item in items.iter().cloned() {
                ItemRow { key: "{item.id}", item, loose }
            }
        }
    }
}

/// Пункт: отметка «готово», текст до трёх строк (нажатие показывает целиком),
/// метаданные, если есть, и ▶ «Взять в работу».
#[component]
fn ItemRow(item: TaskDto, loose: bool) -> Element {
    let api = use_api();
    let mut app = use_app();
    let nav = use_navigator();
    let id = item.id;
    let done = is_done(&item);
    let active = app.status.read().as_ref().and_then(|s| s.active.clone());
    let in_work = matches!(&active, Some(ActiveSession::Work { item: it, .. }) if it.id == id);
    let can_start = matches!(active, None | Some(ActiveSession::Planning { .. }));
    let api_done = api.clone();
    let api_start = api.clone();
    let mut expanded = use_signal(|| false);
    let estimate = item.estimate_intervals.filter(|n| *n > 0);
    let today = item.planned_for == Some(fmt::local_date(Utc::now()));
    let has_note = item.notes.as_deref().is_some_and(|n| !n.trim().is_empty());
    let has_meta = in_work || estimate.is_some() || today || has_note;
    // Пункт без родителя можно разбить на пункты — тогда он станет задачей.
    let mut adding = use_signal(|| false);
    let mut draft = use_signal(String::new);
    let mut draft_reset = use_signal(|| 0u32);
    let api_add = api.clone();
    let mut add = move || {
        let Some(cmd) = add_items(id, &draft.peek()) else {
            adding.set(false);
            return;
        };
        run(api_add.clone(), app, cmd, move || {
            draft.set(String::new());
            draft_reset += 1;
        });
    };
    let row = if in_work {
        "item-row in-work"
    } else if done {
        "item-row done"
    } else {
        "item-row"
    };

    rsx! {
        div { class: row,
            Checkbox {
                checked: done,
                label: item.title.clone(),
                on_toggle: move |v| run(api_done.clone(), app, Command::SetDone(id, v), || {}),
            }
            div { class: "item-body",
                button {
                    class: if expanded() { "item-text expanded" } else { "item-text" },
                    "aria-expanded": "{expanded()}",
                    onclick: move |_| expanded.toggle(),
                    "{item.title}"
                }
                if has_meta {
                    div { class: "item-meta",
                        if in_work {
                            span { class: "item-live", span { class: "pulse" } "В работе" }
                        }
                        if let Some(n) = estimate {
                            span { class: "item-estimate", title: "Оценка: {n} × 50 минут",
                                span { class: "dots", "aria-hidden": "true",
                                    for k in 0..n.min(6) {
                                        i { key: "{k}" }
                                    }
                                }
                                span { class: "num", "{n} × 50 мин" }
                            }
                        }
                        if today {
                            span { class: "item-today", "сегодня" }
                        }
                        if has_note {
                            span { class: "item-note", title: "{item.notes.clone().unwrap_or_default()}",
                                {Icon::Note.render(13)}
                                "заметка"
                            }
                        }
                    }
                }
                if expanded() && loose && !done && !in_work && !adding() {
                    div { class: "item-actions",
                        button {
                            class: "btn btn-ghost btn-small",
                            title: "Пункт станет задачей",
                            onclick: move |_| adding.set(true),
                            {Icon::Plus.render(14)}
                            "Разбить на пункты"
                        }
                    }
                }
            }
            if in_work {
                span { class: "play-slot", "aria-hidden": "true" }
            } else if done {
                span { class: "play-slot" }
            } else {
                button {
                    class: "play",
                    disabled: !can_start || (app.busy)(),
                    title: if can_start { "Взять в работу — 50 минут" } else { "Сначала закончи текущую сессию" },
                    "aria-label": "Взять в работу: {item.title}",
                    onclick: move |_| {
                        app.selected_item.set(Some(id));
                        let nav = nav;
                        run(api_start.clone(), app, Command::StartWork(id), move || {
                            app.selected_item.set(None);
                            nav.push(Route::Focus {});
                        });
                    },
                    span { class: "play-dot", {Icon::Play.render(14)} }
                }
            }
        }
        if adding() {
            div { class: "item-add",
                TextArea {
                    reset: draft_reset(),
                    refocus: true,
                    class: "inline-input",
                    placeholder: "Пункт для «{item.title}» — Enter, чтобы добавить",
                    label: "Новый пункт",
                    autofocus: true,
                    on_input: move |v| draft.set(v),
                    on_keydown: move |e: KeyboardEvent| match e.key() {
                        Key::Enter if !shift(&e) => {
                            e.prevent_default();
                            add();
                        }
                        Key::Escape => {
                            draft.set(String::new());
                            adding.set(false);
                        }
                        _ => {}
                    },
                    on_blur: move |_| {
                        if draft.peek().trim().is_empty() {
                            adding.set(false);
                        }
                    },
                }
            }
        }
    }
}

/// Отметка «готово»: квадрат 18 px в зоне нажатия 44 px.
#[component]
fn Checkbox(checked: bool, label: String, on_toggle: EventHandler<bool>) -> Element {
    rsx! {
        button {
            class: "check-btn",
            role: "checkbox",
            "aria-checked": "{checked}",
            "aria-label": "Готово: {label}",
            onclick: move |_| on_toggle.call(!checked),
            span { class: if checked { "check checked" } else { "check" },
                if checked {
                    {Icon::Check.render(14)}
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn draft_kinds() {
        assert_eq!(parse_draft("  \n "), Draft::Empty);
        assert_eq!(
            parse_draft(" Обновить README "),
            Draft::Text("Обновить README".into())
        );
        assert_eq!(
            parse_draft("https://jira.example.com/browse/PROJ-456"),
            Draft::Jira(
                "https://jira.example.com/browse/PROJ-456".into(),
                "PROJ-456".into()
            )
        );
        assert_eq!(
            parse_draft("PROJ-7"),
            Draft::Jira("PROJ-7".into(), "PROJ-7".into())
        );
        // Ключ внутри названия — это название, а не импорт.
        assert_eq!(
            parse_draft("Починить PROJ-12 до релиза"),
            Draft::Text("Починить PROJ-12 до релиза".into())
        );
        assert_eq!(
            parse_draft("Отчёт\n\n Схема \nМиграции\n"),
            Draft::List("Отчёт".into(), vec!["Схема".into(), "Миграции".into()])
        );
    }

    #[test]
    fn add_items_by_lines() {
        let id = TaskId::from(focus_api_types::Uuid::nil());
        assert_eq!(add_items(id, " \n"), None);
        assert_eq!(
            add_items(id, "Один"),
            Some(Command::Create(NewTask::child(id, "Один".to_string())))
        );
        assert_eq!(
            add_items(id, "Один\nДва"),
            Some(Command::CreateItems(id, vec!["Один".into(), "Два".into()]))
        );
    }
}
