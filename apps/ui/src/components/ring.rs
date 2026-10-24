//! Кольцо прогресса таймера (SVG).

use dioxus::prelude::*;

/// Цвет кольца.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RingTone {
    /// Работа — синий.
    Work,
    /// Отдых — зелёный.
    Rest,
    /// Планирование — янтарный.
    Planning,
    /// Ничего не идёт.
    Idle,
}

impl RingTone {
    fn class(self) -> &'static str {
        match self {
            RingTone::Work => "work",
            RingTone::Rest => "rest",
            RingTone::Planning => "planning",
            RingTone::Idle => "idle",
        }
    }
}

const R: f64 = 140.0;

/// Кольцо: `progress` 0..=1 — заполненная доля; в центре — `children`.
#[component]
pub fn Ring(tone: RingTone, progress: f64, children: Element) -> Element {
    let c = 2.0 * std::f64::consts::PI * R;
    let p = progress.clamp(0.0, 1.0);
    let offset = c * (1.0 - p);
    // Точка на конце дуги: от 12 часов по часовой стрелке.
    let angle = p * std::f64::consts::TAU - std::f64::consts::FRAC_PI_2;
    let (dx, dy) = (160.0 + R * angle.cos(), 160.0 + R * angle.sin());
    let show_head = tone != RingTone::Idle && p > 0.0 && p < 1.0;
    rsx! {
        div { class: "ring ring-{tone.class()}",
            svg {
                class: "ring-svg",
                view_box: "0 0 320 320",
                "aria-hidden": "true",
                circle { class: "ring-track", cx: "160", cy: "160", r: "{R}" }
                circle {
                    class: "ring-progress",
                    cx: "160",
                    cy: "160",
                    r: "{R}",
                    stroke_dasharray: "{c:.2}",
                    stroke_dashoffset: "{offset:.2}",
                    transform: "rotate(-90 160 160)",
                }
                if show_head {
                    circle { class: "ring-head", cx: "{dx:.2}", cy: "{dy:.2}", r: "5" }
                }
            }
            div { class: "ring-center", {children} }
        }
    }
}
