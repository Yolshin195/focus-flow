//! Циферблат таймера (SVG): по риске на минуту и тонкая дуга по секундам.
//!
//! Риски — главный образ интерфейса: у работы их 50, у отдыха 10. Пока идёт
//! отсчёт вниз, горят оставшиеся минуты и гаснут по одной; у планирования
//! (отсчёт вверх) риски загораются.

use dioxus::prelude::*;

/// Цвет циферблата.
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
    /// CSS-модификатор.
    pub fn class(self) -> &'static str {
        match self {
            RingTone::Work => "work",
            RingTone::Rest => "rest",
            RingTone::Planning => "planning",
            RingTone::Idle => "idle",
        }
    }
}

const C: f64 = 160.0;
const ARC_R: f64 = 128.0;
const TICK_OUT: f64 = 152.0;
const TICK_IN: f64 = 141.0;
const TICK_IN_MAJOR: f64 = 136.0;

/// Циферблат: `ticks` — сколько минут на круге, `lit` — доля 0..=1 горящей
/// части (по часовой стрелке от 12 часов); в центре — `children`.
#[component]
pub fn Ring(tone: RingTone, ticks: u32, lit: f64, children: Element) -> Element {
    let ticks = ticks.clamp(1, 120);
    let lit = lit.clamp(0.0, 1.0);
    let len = 2.0 * std::f64::consts::PI * ARC_R;
    let offset = len * (1.0 - lit);
    let lit_ticks = (lit * f64::from(ticks)).ceil() as u32;
    // Каждая пятая риска длиннее — как на шкале прибора.
    let marks: Vec<(String, &'static str)> = (0..ticks)
        .map(|i| {
            let a = f64::from(i) / f64::from(ticks) * std::f64::consts::TAU
                - std::f64::consts::FRAC_PI_2;
            let major = ticks > 12 && i % 5 == 0;
            let r_in = if major { TICK_IN_MAJOR } else { TICK_IN };
            let (c, s) = (a.cos(), a.sin());
            let d = format!(
                "M{:.2} {:.2}L{:.2} {:.2}",
                C + r_in * c,
                C + r_in * s,
                C + TICK_OUT * c,
                C + TICK_OUT * s
            );
            let class = match (i < lit_ticks, i + 1 == lit_ticks) {
                (true, true) => "tick on edge",
                (true, false) => "tick on",
                _ => "tick",
            };
            (d, class)
        })
        .collect();
    rsx! {
        div { class: "ring ring-{tone.class()}",
            svg {
                class: "ring-svg",
                view_box: "0 0 320 320",
                "aria-hidden": "true",
                g { class: "ticks",
                    for (i, (d, class)) in marks.iter().enumerate() {
                        path { key: "{i}", class: "{class}", d: "{d}" }
                    }
                }
                circle { class: "ring-track", cx: "{C}", cy: "{C}", r: "{ARC_R}" }
                circle {
                    class: "ring-progress",
                    cx: "{C}",
                    cy: "{C}",
                    r: "{ARC_R}",
                    stroke_dasharray: "{len:.2}",
                    stroke_dashoffset: "{offset:.2}",
                    transform: "rotate(-90 {C} {C})",
                }
            }
            div { class: "ring-center", {children} }
        }
    }
}
