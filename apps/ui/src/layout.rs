//! Каркас: узкая боковая панель с навигацией и прогрессом дня + экран.

use dioxus::prelude::*;

use crate::components::Icon;
use crate::fmt;
use crate::state::{use_app, UiPreset};
use crate::Route;

/// Общий layout всех экранов.
#[component]
pub fn Layout() -> Element {
    let mut app = use_app();
    let nav = use_navigator();
    // Пресет демо: сразу открыть нужный экран (один раз).
    use_hook(move || match *app.preset.peek() {
        UiPreset::Tasks => {
            nav.replace(Route::Tasks {});
        }
        UiPreset::Day => {
            nav.replace(Route::Day {});
        }
        _ => {}
    });
    let (worked, target) = app
        .status
        .read()
        .as_ref()
        .map(|s| (s.worked_today_sec, s.target_sec))
        .unwrap_or((0, 8 * 3600));
    let pct = (worked as f64 / target.max(1) as f64 * 100.0).min(100.0);
    let error = app.error.read().clone();

    rsx! {
        div { class: "shell",
            nav { class: "sidebar", "aria-label": "Разделы",
                div { class: "brand", title: "Focus Flow",
                    svg { class: "brand-mark", width: "28", height: "28", view_box: "0 0 28 28", "aria-hidden": "true",
                        circle { cx: "14", cy: "14", r: "11", class: "brand-track" }
                        path { d: "M14 3a11 11 0 0 1 10.46 14.4", class: "brand-arc" }
                    }
                }
                div { class: "nav",
                    NavItem { to: Route::Focus {}, icon: Icon::Focus, label: "Фокус" }
                    NavItem { to: Route::Tasks {}, icon: Icon::Tasks, label: "Задачи" }
                    NavItem { to: Route::Day {}, icon: Icon::Day, label: "День" }
                }
                div { class: "side-progress", title: "Норма дня",
                    div { class: "side-progress-value num", "{fmt::hm(worked)}" }
                    div { class: "side-progress-of", "из " span { class: "num", "{fmt::hm(target)}" } }
                    div { class: "side-progress-bar",
                        div { class: "side-progress-fill", style: "width: {pct:.2}%" }
                    }
                }
            }
            main { class: "screen",
                Outlet::<Route> {}
            }
            if let Some(err) = error {
                div { class: "toast", role: "alert",
                    span { "{err}" }
                    button {
                        class: "icon-btn",
                        "aria-label": "Закрыть",
                        onclick: move |_| app.error.set(None),
                        {Icon::Close.render(16)}
                    }
                }
            }
        }
    }
}

#[component]
fn NavItem(to: Route, icon: Icon, label: &'static str) -> Element {
    rsx! {
        Link { class: "nav-item", active_class: "active", to,
            {icon.render(20)}
            span { class: "nav-label", "{label}" }
        }
    }
}
