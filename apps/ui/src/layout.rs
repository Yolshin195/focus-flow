//! Каркас: узкая боковая панель с навигацией и прогрессом дня + экран.

use dioxus::prelude::*;

use crate::components::Icon;
use crate::fmt;
use crate::state::{use_app, Conn, UiPreset};
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
        UiPreset::Settings => {
            nav.replace(Route::Settings {});
        }
        _ => {}
    });
    let norm = app
        .status
        .read()
        .as_ref()
        .map(|s| (s.worked_today_sec, s.target_sec));
    let pct = norm
        .map(|(worked, target)| (worked as f64 / target.max(1) as f64 * 100.0).min(100.0))
        .unwrap_or(0.0);
    let conn = app.conn.read().clone();
    // Пока нет связи, тост «Нет связи…» не нужен — об этом говорит плашка.
    let error = app
        .error
        .read()
        .clone()
        .filter(|_| !matches!(conn, Conn::Offline | Conn::Unauthorized));

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
                div { class: "nav nav-bottom",
                    NavItem { to: Route::Settings {}, icon: Icon::Settings, label: "Настройки" }
                }
                div { class: "side-progress", title: "Норма дня",
                    if let Some((worked, target)) = norm {
                        div { class: "side-progress-value num", "{fmt::hm(worked)}" }
                        div { class: "side-progress-of", "из " span { class: "num", "{fmt::hm(target)}" } }
                    } else {
                        // До данных — пустые строки той же высоты, без выдуманных 0:00.
                        div { class: "side-progress-value num", {fmt::NBSP} }
                        div { class: "side-progress-of", {fmt::NBSP} }
                    }
                    div { class: "side-progress-bar",
                        div { class: "side-progress-fill", style: "width: {pct:.2}%" }
                    }
                }
            }
            main { class: "screen",
                ConnBanner { conn }
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

/// Плашка «Нет связи с сервисом»: тонкая полоса сверху экрана, не мешает
/// смотреть последние данные. Переподключение идёт само.
#[component]
fn ConnBanner(conn: Conn) -> Element {
    let (title, hint, cmd) = match conn {
        Conn::Offline => (
            "Нет связи с сервисом",
            "Переподключаюсь… Запусти:",
            "focus-flow serve",
        ),
        Conn::Unauthorized => (
            "Сервис не принял токен",
            "Проверь FOCUS_TOKEN или файл токена:",
            "focus-flow token",
        ),
        Conn::Unknown | Conn::Online => return rsx! {},
    };
    rsx! {
        div { class: "conn-banner", role: "status",
            span { class: "conn-dot" }
            span { class: "conn-title", "{title}" }
            span { class: "conn-hint", "{hint}" }
            code { class: "conn-cmd", "{cmd}" }
        }
    }
}
