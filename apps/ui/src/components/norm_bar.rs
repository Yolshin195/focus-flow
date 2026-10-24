//! Тонкая полоса нормы дня, разбитая на отрезки по видам сессий.

use dioxus::prelude::*;

/// Отрезок полосы: CSS-класс вида и длительность в секундах.
#[derive(Debug, Clone, PartialEq)]
pub struct Segment {
    /// `work` | `planning`.
    pub class: &'static str,
    /// Секунды.
    pub sec: i64,
}

/// Полоса нормы: сумма отрезков относительно `target_sec`.
#[component]
pub fn NormBar(segments: Vec<Segment>, target_sec: i64) -> Element {
    let target = target_sec.max(1) as f64;
    rsx! {
        div {
            class: "norm-bar",
            role: "progressbar",
            "aria-valuemin": "0",
            "aria-valuemax": "{target_sec}",
            "aria-valuenow": "{segments.iter().map(|s| s.sec).sum::<i64>()}",
            for (i, s) in segments.iter().enumerate() {
                div {
                    key: "{i}",
                    class: "norm-seg {s.class}",
                    style: "width: {(s.sec as f64 / target * 100.0).min(100.0):.3}%",
                }
            }
        }
    }
}
