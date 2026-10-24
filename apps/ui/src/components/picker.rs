//! Выбор пункта: поиск + открытые пункты, сгруппированные по задачам.

use dioxus::prelude::*;
use focus_api_types::{TaskDto, TaskId, TaskStatus};

use super::Icon;
use crate::api::jira_key;

/// Группа пунктов под задачей.
#[derive(Debug, Clone, PartialEq)]
struct Group {
    title: String,
    key: Option<String>,
    items: Vec<(TaskId, String)>,
}

fn is_open(t: &TaskDto) -> bool {
    !matches!(t.status, TaskStatus::Done | TaskStatus::Cancelled)
}

fn groups(tree: &[TaskDto], query: &str) -> Vec<Group> {
    let q = query.trim().to_lowercase();
    let hit = |s: &str| q.is_empty() || s.to_lowercase().contains(&q);
    let mut out = Vec::new();
    let mut loose = Vec::new();
    for t in tree.iter().filter(|t| is_open(t)) {
        if t.is_item() {
            if hit(&t.title) {
                loose.push((t.id, t.title.clone()));
            }
            continue;
        }
        let key = t.external_url.as_deref().and_then(jira_key);
        let parent_hit = hit(&t.title) || key.as_deref().is_some_and(hit);
        let items: Vec<_> = t
            .children
            .iter()
            .filter(|c| is_open(c) && (parent_hit || hit(&c.title)))
            .map(|c| (c.id, c.title.clone()))
            .collect();
        if !items.is_empty() {
            out.push(Group {
                title: t.title.clone(),
                key,
                items,
            });
        }
    }
    if !loose.is_empty() {
        out.push(Group {
            title: "Без задачи".into(),
            key: None,
            items: loose,
        });
    }
    out
}

/// Список открытых пунктов. Выбор пишет в `selected`; двойной клик/Enter — `on_pick`.
#[component]
pub fn ItemPicker(
    tasks: ReadSignal<Vec<TaskDto>>,
    selected: Signal<Option<TaskId>>,
    on_pick: EventHandler<TaskId>,
) -> Element {
    let mut query = use_signal(String::new);
    let list = use_memo(move || groups(&tasks.read(), &query.read()));
    let flat: Vec<TaskId> = list
        .read()
        .iter()
        .flat_map(|g| g.items.iter().map(|(id, _)| *id))
        .collect();
    let total = flat.len();

    let mut selected = selected;
    let flat_nav = flat.clone();
    let onkeydown = move |e: KeyboardEvent| {
        let pos = selected().and_then(|s| flat_nav.iter().position(|x| *x == s));
        let next = match e.key() {
            Key::ArrowDown => {
                Some(pos.map_or(0, |p| (p + 1).min(flat_nav.len().saturating_sub(1))))
            }
            Key::ArrowUp => Some(pos.map_or(0, |p| p.saturating_sub(1))),
            Key::Enter => {
                if let Some(id) = selected() {
                    e.prevent_default();
                    e.stop_propagation();
                    on_pick.call(id);
                }
                None
            }
            _ => None,
        };
        if let Some(i) = next {
            e.prevent_default();
            e.stop_propagation();
            if let Some(id) = flat_nav.get(i) {
                selected.set(Some(*id));
            }
        }
    };

    rsx! {
        div { class: "picker", onkeydown,
            label { class: "picker-search",
                {Icon::Search.render(16)}
                input {
                    r#type: "text",
                    placeholder: "Найти пункт",
                    value: "{query}",
                    autofocus: true,
                    oninput: move |e| query.set(e.value()),
                    onkeydown: move |e: KeyboardEvent| {
                        // Пробел в поиске — это текст, а не «главное действие».
                        if e.key() == Key::Character(" ".into()) {
                            e.stop_propagation();
                        }
                    },
                }
            }
            div { class: "picker-list", role: "listbox", "aria-label": "Открытые пункты",
                if total == 0 {
                    p { class: "picker-empty",
                        if query.read().is_empty() {
                            "Открытых пунктов нет. Добавь их на экране «Задачи»."
                        } else {
                            "Ничего не нашлось."
                        }
                    }
                }
                for g in list.read().iter() {
                    div { class: "picker-group", key: "{g.title}",
                        div { class: "picker-group-title",
                            if let Some(k) = &g.key {
                                span { class: "chip chip-jira", "{k}" }
                            }
                            span { class: "picker-group-name", "{g.title}" }
                        }
                        for (id, title) in g.items.iter().cloned() {
                            button {
                                key: "{id}",
                                class: if selected() == Some(id) { "picker-item selected" } else { "picker-item" },
                                role: "option",
                                "aria-selected": "{selected() == Some(id)}",
                                onclick: move |_| selected.set(Some(id)),
                                ondoubleclick: move |_| on_pick.call(id),
                                span { class: "picker-dot" }
                                span { class: "picker-item-title", "{title}" }
                            }
                        }
                    }
                }
            }
        }
    }
}
