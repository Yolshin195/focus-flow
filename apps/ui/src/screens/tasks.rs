//! Экран «Задачи»: дерево задач → пункты, быстрый ввод, импорт из Jira.

use std::collections::HashSet;

use dioxus::prelude::*;
use focus_api_types::{ActiveSession, ApiError, NewTask, TaskDto, TaskId, TaskStatus};

use crate::api::jira_key;
use crate::components::Icon;
use crate::fmt;
use crate::state::{run, use_api, use_app, Command};
use crate::Route;

fn is_done(t: &TaskDto) -> bool {
    matches!(t.status, TaskStatus::Done | TaskStatus::Cancelled)
}

/// Экран «Задачи».
#[component]
pub fn Tasks() -> Element {
    let api = use_api();
    let app = use_app();
    let mut title = use_signal(String::new);
    let mut link = use_signal(String::new);
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

    let api_create = api.clone();
    let create = move || {
        let t = title.peek().trim().to_string();
        if t.is_empty() {
            return;
        }
        run(
            api_create.clone(),
            app,
            Command::Create(NewTask::new(t)),
            move || {
                title.set(String::new());
            },
        );
    };
    let create_enter = create.clone();
    let create_click = create;

    let api_import = api.clone();
    let import = move || {
        let url = link.peek().trim().to_string();
        if url.is_empty() {
            return;
        }
        run(api_import.clone(), app, Command::Import(url), move || {
            link.set(String::new());
        });
    };
    let import_enter = import.clone();
    let import_click = import;
    let link_valid = jira_key(&link.read()).is_some();
    // Подключена ли Jira: без неё импорт не сработает — сразу скажем об этом
    // и дадим ссылку на настройки. Ошибка запроса — считаем, что неизвестно.
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
                    input {
                        r#type: "text",
                        placeholder: "Новая задача…",
                        value: "{title}",
                        "aria-label": "Новая задача",
                        oninput: move |e| title.set(e.value()),
                        onkeydown: move |e: KeyboardEvent| {
                            if e.key() == Key::Enter {
                                create_enter();
                            }
                        },
                    }
                    if !title.read().trim().is_empty() {
                        button { class: "btn btn-small", onclick: move |_| create_click(), "Создать" }
                    }
                }
                div { class: "field field-import",
                    input {
                        r#type: "text",
                        placeholder: if jira_ready { "Вставь ссылку на задачу Jira" } else { "Импорт из Jira" },
                        value: "{link}",
                        "aria-label": "Ссылка на задачу Jira",
                        oninput: move |e| link.set(e.value()),
                        onkeydown: move |e: KeyboardEvent| {
                            if e.key() == Key::Enter && jira_ready {
                                import_enter();
                            }
                        },
                    }
                    button {
                        class: "btn btn-small",
                        disabled: !link_valid || busy || !jira_ready,
                        onclick: move |_| import_click(),
                        "Импорт"
                    }
                }
                if let Some(note) = jira_note {
                    p { class: "import-note", role: "note",
                        "{note} "
                        Link { to: Route::Settings {}, "Подключить в настройках" }
                    }
                }
            }

            if tree.is_empty() {
                div { class: "empty",
                    h2 { class: "empty-title", "Задач пока нет" }
                    p { class: "empty-text",
                        "Напиши задачу в поле выше и нажми Enter или вставь ссылку на задачу Jira — пункты чек-листа станут шагами."
                    }
                }
            }

            div { class: "tree",
                for task in tree.iter().filter(|t| !t.is_item()).cloned() {
                    TaskNode { key: "{task.id}", task, collapsed }
                }
                if !loose.is_empty() {
                    div { class: "task task-loose",
                        div { class: "task-head",
                            span { class: "task-title", "Без задачи" }
                        }
                        div { class: "items",
                            for item in loose.iter().cloned() {
                                ItemRow { key: "{item.id}", item, loose: true }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn TaskNode(task: TaskDto, collapsed: Signal<HashSet<TaskId>>) -> Element {
    let api = use_api();
    let app = use_app();
    let mut collapsed = collapsed;
    let mut adding = use_signal(|| false);
    let mut draft = use_signal(String::new);
    let id = task.id;
    let is_collapsed = collapsed.read().contains(&id);
    let key = task.external_url.as_deref().and_then(jira_key);
    let done = is_done(&task);
    let total = task.children.len();
    let closed = task.children.iter().filter(|c| is_done(c)).count();

    let api_add = api.clone();
    let mut add = move || {
        let t = draft.peek().trim().to_string();
        if t.is_empty() {
            adding.set(false);
            return;
        }
        run(
            api_add.clone(),
            app,
            Command::Create(NewTask::child(id, t)),
            move || {
                draft.set(String::new());
            },
        );
    };
    let api_done = api.clone();

    rsx! {
        div { class: if done { "task done" } else { "task" },
            div { class: "task-head",
                button {
                    class: if is_collapsed { "icon-btn caret collapsed" } else { "icon-btn caret" },
                    "aria-label": if is_collapsed { "Развернуть" } else { "Свернуть" },
                    "aria-expanded": "{!is_collapsed}",
                    onclick: move |_| {
                        let mut set = collapsed.write();
                        if !set.remove(&id) {
                            set.insert(id);
                        }
                    },
                    {Icon::Chevron.render(16)}
                }
                Checkbox {
                    checked: done,
                    label: task.title.clone(),
                    on_toggle: move |v| run(api_done.clone(), app, Command::SetDone(id, v), || {}),
                }
                div { class: "task-title-wrap",
                    span { class: "task-title", "{task.title}" }
                    if let (Some(k), Some(url)) = (&key, &task.external_url) {
                        a { class: "chip chip-jira chip-link", href: "{url}", target: "_blank", title: "Открыть в Jira",
                            "{k}"
                            {Icon::External.render(12)}
                        }
                    }
                }
                if total > 0 {
                    span { class: "task-count num", "{closed}/{total}" }
                }
                button {
                    class: "btn btn-ghost btn-small",
                    onclick: move |_| {
                        adding.set(true);
                        collapsed.write().remove(&id);
                    },
                    {Icon::Plus.render(14)}
                    "Пункт"
                }
            }
            if !is_collapsed {
                div { class: "items",
                    for item in task.children.iter().cloned() {
                        ItemRow { key: "{item.id}", item, loose: false }
                    }
                    if adding() {
                        div { class: "item-add",
                            input {
                                r#type: "text",
                                class: "inline-input",
                                placeholder: "Новый пункт — Enter, чтобы добавить",
                                value: "{draft}",
                                autofocus: true,
                                oninput: move |e| draft.set(e.value()),
                                onkeydown: move |e: KeyboardEvent| match e.key() {
                                    Key::Enter => add(),
                                    Key::Escape => {
                                        draft.set(String::new());
                                        adding.set(false);
                                    }
                                    _ => {}
                                },
                                onblur: move |_| {
                                    if draft.peek().trim().is_empty() {
                                        adding.set(false);
                                    }
                                },
                            }
                        }
                    } else if total == 0 {
                        p { class: "items-empty", "Пунктов нет. Разбей задачу на шаги по 50 минут." }
                    }
                }
            }
        }
    }
}

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

    rsx! {
        div {
            class: if in_work { "item-row in-work" } else if done { "item-row done" } else { "item-row" },
            class: if loose { "loose" },
            Checkbox {
                checked: done,
                label: item.title.clone(),
                on_toggle: move |v| run(api_done.clone(), app, Command::SetDone(id, v), || {}),
            }
            span { class: "item-row-title", "{item.title}" }
            if in_work {
                span { class: "badge badge-work", span { class: "pulse" } "В работе" }
            } else if !done {
                button {
                    class: "btn btn-ghost btn-small take",
                    disabled: !can_start || (app.busy)(),
                    title: if can_start { "Начать 50 минут над этим пунктом" } else { "Сначала закончи текущую сессию" },
                    onclick: move |_| {
                        app.selected_item.set(Some(id));
                        let nav = nav;
                        run(api_start.clone(), app, Command::StartWork(id), move || {
                            app.selected_item.set(None);
                            nav.push(Route::Focus {});
                        });
                    },
                    {Icon::Play.render(12)}
                    "Взять в работу"
                }
            }
        }
    }
}

#[component]
fn Checkbox(checked: bool, label: String, on_toggle: EventHandler<bool>) -> Element {
    rsx! {
        button {
            class: if checked { "check checked" } else { "check" },
            role: "checkbox",
            "aria-checked": "{checked}",
            "aria-label": "Готово: {label}",
            onclick: move |_| on_toggle.call(!checked),
            if checked {
                {Icon::Check.render(14)}
            }
        }
    }
}
