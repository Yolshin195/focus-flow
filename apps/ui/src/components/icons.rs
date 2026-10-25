//! Простые иконки (inline SVG, 20×20, обводка `currentColor`).

use dioxus::prelude::*;

/// Какая иконка.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Icon {
    /// Фокус — мишень.
    Focus,
    /// Задачи — список.
    Tasks,
    /// День — шкала.
    Day,
    /// Шеврон вниз.
    Chevron,
    /// Плюс.
    Plus,
    /// Минус.
    Minus,
    /// Поиск.
    Search,
    /// Галочка.
    Check,
    /// Внешняя ссылка.
    External,
    /// Закрыть.
    Close,
    /// Play.
    Play,
    /// Отдых — чашка.
    Rest,
    /// Планирование — список с карандашом.
    Plan,
    /// Настройки — ползунки.
    Settings,
    /// Ещё — три точки.
    More,
    /// Заметка — лист с строками.
    Note,
    /// Синхронизация — две стрелки по кругу.
    Sync,
}

#[component]
fn Svg(children: Element, #[props(default = 20)] size: u32) -> Element {
    rsx! {
        svg {
            class: "icon",
            width: "{size}",
            height: "{size}",
            view_box: "0 0 20 20",
            fill: "none",
            stroke: "currentColor",
            stroke_width: "1.6",
            stroke_linecap: "round",
            stroke_linejoin: "round",
            "aria-hidden": "true",
            {children}
        }
    }
}

impl Icon {
    /// Рендер иконки размера `size`.
    pub fn render(self, size: u32) -> Element {
        match self {
            Icon::Focus => rsx! {
                Svg { size,
                    circle { cx: "10", cy: "10", r: "7" }
                    circle { cx: "10", cy: "10", r: "3" }
                }
            },
            Icon::Tasks => rsx! {
                Svg { size,
                    path { d: "M8 5.5h8M8 10h8M8 14.5h5" }
                    path { d: "M3.5 5.5l1 1 1.8-2M3.5 10h2.5M3.5 14.5h2.5" }
                }
            },
            Icon::Day => rsx! {
                Svg { size,
                    path { d: "M5 3v14" }
                    rect { x: "8", y: "4", width: "8", height: "4", rx: "1.2" }
                    rect { x: "8", y: "11", width: "5", height: "5", rx: "1.2" }
                }
            },
            Icon::Chevron => rsx! {
                Svg { size, path { d: "M6 8l4 4 4-4" } }
            },
            Icon::Plus => rsx! {
                Svg { size, path { d: "M10 4.5v11M4.5 10h11" } }
            },
            Icon::Minus => rsx! {
                Svg { size, path { d: "M4.5 10h11" } }
            },
            Icon::Sync => rsx! {
                Svg { size,
                    path { d: "M15.6 8.2A6 6 0 0 0 4.9 6" }
                    path { d: "M4.5 3.5V6.5h3" }
                    path { d: "M4.4 11.8A6 6 0 0 0 15.1 14" }
                    path { d: "M15.5 16.5v-3h-3" }
                }
            },
            Icon::Search => rsx! {
                Svg { size,
                    circle { cx: "9", cy: "9", r: "5" }
                    path { d: "M13 13l3.5 3.5" }
                }
            },
            Icon::Check => rsx! {
                Svg { size, path { d: "M5 10.5l3.2 3L15 6.5" } }
            },
            Icon::External => rsx! {
                Svg { size,
                    path { d: "M11 4h5v5M16 4l-7 7" }
                    path { d: "M14 12v3.5a.5.5 0 0 1-.5.5h-9a.5.5 0 0 1-.5-.5v-9a.5.5 0 0 1 .5-.5H8" }
                }
            },
            Icon::Close => rsx! {
                Svg { size, path { d: "M5.5 5.5l9 9M14.5 5.5l-9 9" } }
            },
            Icon::Rest => rsx! {
                Svg { size,
                    path { d: "M4 8h10v4a4 4 0 0 1-4 4H8a4 4 0 0 1-4-4z" }
                    path { d: "M14 9h1.2a2 2 0 0 1 0 4H14" }
                    path { d: "M7.5 3.5c-.6.8.6 1.4 0 2.5M10.5 3.5c-.6.8.6 1.4 0 2.5" }
                }
            },
            Icon::Plan => rsx! {
                Svg { size,
                    path { d: "M4 5h7M4 9h5M4 13h4" }
                    path { d: "M15.5 7.5l1.5 1.5-6 6-2.2.7.7-2.2z" }
                }
            },
            Icon::Settings => rsx! {
                Svg { size,
                    path { d: "M4 6h6M14 6h2M4 14h2M10 14h6" }
                    circle { cx: "12", cy: "6", r: "2" }
                    circle { cx: "8", cy: "14", r: "2" }
                }
            },
            Icon::Play => rsx! {
                Svg { size, path { d: "M7 5l8 5-8 5z" } }
            },
            Icon::More => rsx! {
                Svg { size,
                    circle { cx: "5", cy: "10", r: "0.9" }
                    circle { cx: "10", cy: "10", r: "0.9" }
                    circle { cx: "15", cy: "10", r: "0.9" }
                }
            },
            Icon::Note => rsx! {
                Svg { size,
                    rect { x: "5", y: "3.5", width: "10", height: "13", rx: "1.5" }
                    path { d: "M8 8h4M8 11h4M8 14h2" }
                }
            },
        }
    }
}
