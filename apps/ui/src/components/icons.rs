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
            Icon::Play => rsx! {
                Svg { size, path { d: "M7 5l8 5-8 5z" } }
            },
        }
    }
}
