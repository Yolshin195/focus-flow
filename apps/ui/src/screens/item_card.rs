//! Карточка пункта: полный текст, заметка, оценка, «на сегодня»,
//! «Взять в работу», «Отменить». Правки — `update_task` (поля `TaskPatch`).

use dioxus::prelude::*;
use focus_api_types::chrono::Utc;
use focus_api_types::{ActiveSession, TaskDto, TaskPatch, TaskStatus};

use crate::api::jira_key;
use crate::components::{Icon, TextArea};
use crate::fmt;
use crate::state::{run, use_api, use_app, Command};
use crate::Route;

/// Самая большая оценка: шесть интервалов — уже повод разбить пункт.
const MAX_INTERVALS: u32 = 6;

/// Enter в поле текста сохраняет его — снимаем фокус, сработает `onblur`.
const BLUR_JS: &str = "document.activeElement?.blur();";

/// Карточка. `parent` — задача пункта (`None` — пункт без задачи).
#[component]
pub fn ItemCard(item: TaskDto, parent: Option<TaskDto>, on_close: EventHandler<()>) -> Element {
    let api = use_api();
    let mut app = use_app();
    let nav = use_navigator();
    let id = item.id;
    let done = matches!(item.status, TaskStatus::Done | TaskStatus::Cancelled);
    // Пункт из чек-листа Jira: плагин находит строку по тексту, поэтому
    // текст правится только в Jira.
    let jira = parent.as_ref().and_then(|p| {
        p.external_url
            .as_deref()
            .and_then(jira_key)
            .zip(p.external_url.clone())
    });
    let estimate = item.estimate_intervals.unwrap_or(0);
    let today = fmt::local_date(Utc::now());
    let planned_today = item.planned_for == Some(today);
    let active = app.status.read().as_ref().and_then(|s| s.active.clone());
    let in_work = matches!(&active, Some(ActiveSession::Work { item: it, .. }) if it.id == id);
    let can_start = matches!(active, None | Some(ActiveSession::Planning { .. }));
    let busy = (app.busy)();
    let mut title = use_signal(|| item.title.clone());
    let mut notes = use_signal(|| item.notes.clone().unwrap_or_default());
    let mut split = use_signal(String::new);

    let patch = {
        let api = api.clone();
        move |p: TaskPatch| run(api.clone(), app, Command::Update(id, p), || {})
    };
    let original_title = item.title.clone();
    let save_title = {
        let patch = patch.clone();
        move || {
            let t = title.peek().trim().to_string();
            if !t.is_empty() && t != original_title {
                patch(TaskPatch {
                    title: Some(t),
                    ..TaskPatch::default()
                });
            }
        }
    };
    let original_notes = item.notes.clone().unwrap_or_default();
    let save_notes = {
        let patch = patch.clone();
        move || {
            let n = notes.peek().trim().to_string();
            if n != original_notes.trim() {
                patch(TaskPatch {
                    notes: Some((!n.is_empty()).then_some(n)),
                    ..TaskPatch::default()
                });
            }
        }
    };
    let set_estimate = {
        let patch = patch.clone();
        move |n: u32| {
            patch(TaskPatch {
                estimate_intervals: Some((n > 0).then_some(n)),
                ..TaskPatch::default()
            })
        }
    };
    let est_less = set_estimate.clone();
    let est_more = set_estimate;
    let toggle_today = {
        let patch = patch.clone();
        move || {
            patch(TaskPatch {
                planned_for: Some((!planned_today).then_some(today)),
                ..TaskPatch::default()
            })
        }
    };
    let cancel = {
        let patch = patch.clone();
        move || {
            patch(TaskPatch {
                status: Some(TaskStatus::Cancelled),
                ..TaskPatch::default()
            });
            on_close.call(());
        }
    };
    let api_reopen = api.clone();
    let api_start = api.clone();
    let api_split = api.clone();

    rsx! {
        div { class: "item-card-head",
            div { class: "item-card-parent",
                if let Some(p) = &parent {
                    span { class: "item-card-parent-title", "{p.title}" }
                    if let Some((key, url)) = &jira {
                        a { class: "chip chip-jira chip-link", href: "{url}", target: "_blank", title: "Открыть в Jira",
                            "{key}"
                            {Icon::External.render(12)}
                        }
                    }
                } else {
                    span { "Без задачи" }
                }
            }
            button {
                class: "icon-btn card-close",
                "aria-label": "Закрыть карточку",
                onclick: move |_| on_close.call(()),
                {Icon::Close.render(18)}
            }
        }

        if jira.is_some() {
            p { class: "item-card-title", "{item.title}" }
            p { class: "item-card-note", "Текст сверяется со строкой чек-листа в Jira — правится там." }
        } else {
            TextArea {
                class: "item-card-title",
                initial: item.title.clone(),
                label: "Текст пункта",
                on_input: move |v| title.set(v),
                on_keydown: move |e: KeyboardEvent| {
                    if e.key() == Key::Enter {
                        e.prevent_default();
                        document::eval(BLUR_JS);
                    }
                },
                on_blur: move |_| save_title(),
            }
        }
        if in_work {
            p { class: "item-card-live", span { class: "pulse" } "Сейчас в работе" }
        }

        div { class: "item-card-rows",
            div { class: "item-card-row",
                div { class: "item-card-row-label",
                    "Оценка"
                    small { "интервалы по 50 минут" }
                }
                div { class: "stepper",
                    button {
                        class: "icon-btn",
                        "aria-label": "Меньше",
                        disabled: estimate == 0 || busy,
                        onclick: move |_| est_less(estimate.saturating_sub(1)),
                        {Icon::Minus.render(16)}
                    }
                    output { class: "stepper-value",
                        if estimate > 0 {
                            span { class: "dots", "aria-hidden": "true",
                                for k in 0..estimate {
                                    i { key: "{k}" }
                                }
                            }
                            span { class: "num", "{estimate} × 50 мин" }
                        } else {
                            span { "нет" }
                        }
                    }
                    button {
                        class: "icon-btn",
                        "aria-label": "Больше",
                        disabled: estimate >= MAX_INTERVALS || busy,
                        onclick: move |_| est_more((estimate + 1).min(MAX_INTERVALS)),
                        {Icon::Plus.render(16)}
                    }
                }
            }
            div { class: "item-card-row",
                div { class: "item-card-row-label",
                    "На сегодня"
                    small { "войдёт в план дня" }
                }
                button {
                    class: if planned_today { "switch on" } else { "switch" },
                    role: "switch",
                    "aria-checked": "{planned_today}",
                    "aria-label": "На сегодня",
                    disabled: busy,
                    onclick: move |_| toggle_today(),
                }
            }
        }

        div { class: "item-card-label", "Заметка" }
        TextArea {
            class: "item-card-notes",
            initial: item.notes.clone().unwrap_or_default(),
            placeholder: "Детали, ссылки, критерий готовности",
            label: "Заметка",
            on_input: move |v| notes.set(v),
            on_blur: move |_| save_notes(),
        }

        div { class: "item-card-actions",
            if done {
                button {
                    class: "btn btn-primary btn-work",
                    disabled: busy,
                    onclick: move |_| run(api_reopen.clone(), app, Command::SetDone(id, false), || {}),
                    "Вернуть в открытые"
                }
            } else if in_work {
                button {
                    class: "btn btn-primary btn-work",
                    onclick: move |_| {
                        on_close.call(());
                        nav.push(Route::Focus {});
                    },
                    "Открыть «Фокус»"
                }
            } else {
                button {
                    class: "btn btn-primary btn-work",
                    disabled: !can_start || busy,
                    title: if can_start { "" } else { "Сначала закончи текущую сессию" },
                    onclick: move |_| {
                        app.selected_item.set(Some(id));
                        let nav = nav;
                        run(api_start.clone(), app, Command::StartWork(id), move || {
                            app.selected_item.set(None);
                            on_close.call(());
                            nav.push(Route::Focus {});
                        });
                    },
                    {Icon::Play.render(16)}
                    "Взять в работу"
                }
                button {
                    class: "btn btn-quiet btn-danger",
                    disabled: busy || in_work,
                    onclick: move |_| cancel(),
                    "Отменить пункт"
                }
                if jira.is_some() {
                    p { class: "item-card-note item-card-note-center", "Отмена и заметка остаются в Focus Flow: строка чек-листа в Jira не изменится." }
                }
            }
        }

        if parent.is_none() && !done && !in_work {
            div { class: "item-card-split",
                div { class: "item-card-label", "Разбить на пункты" }
                TextArea {
                    class: "inline-input",
                    placeholder: "Пункт — Enter; несколько строк — несколько пунктов",
                    label: "Пункты",
                    on_input: move |v| split.set(v),
                    on_keydown: move |e: KeyboardEvent| {
                        if e.key() == Key::Enter && !e.modifiers().contains(Modifiers::SHIFT) {
                            e.prevent_default();
                            let lines: Vec<String> = split
                                .peek()
                                .lines()
                                .map(str::trim)
                                .filter(|l| !l.is_empty())
                                .map(String::from)
                                .collect();
                            if !lines.is_empty() {
                                run(api_split.clone(), app, Command::CreateItems(id, lines), move || {
                                    on_close.call(());
                                });
                            }
                        }
                    },
                }
                p { class: "item-card-note", "Пункт станет задачей, а эти строки — её пунктами." }
            }
        }
    }
}
