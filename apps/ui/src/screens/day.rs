//! Экран «День»: итоги и вертикальная шкала от начала дня до «сейчас».

use dioxus::prelude::*;
use focus_api_types::chrono::{DateTime, Duration, Timelike, Utc};
use focus_api_types::{EndReason, SessionDto, SessionId, SessionKind, TimeRange};

use crate::components::{kind_class, kind_label};
use crate::fmt;
use crate::state::{use_app, UiPreset};

/// Пикселей на минуту шкалы.
const PX_PER_MIN: f64 = 1.6;

fn reason_label(r: Option<EndReason>, kind: SessionKind) -> &'static str {
    match (r, kind) {
        (None, _) => "Идёт сейчас",
        (Some(EndReason::TaskDone), _) => "Пункт готов",
        (Some(EndReason::Limit), SessionKind::Rest) => "Отдых окончен",
        (Some(EndReason::Limit), _) => "Истёк лимит",
        (Some(EndReason::Interrupted), _) => "Прервана",
        (Some(EndReason::Stopped), _) => "Остановлено",
        (Some(EndReason::Switched), _) => "Сменилось сессией",
    }
}

/// Выбранный блок шкалы.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Pick {
    Session(SessionId),
    Lost(usize),
}

/// Экран «День».
#[component]
pub fn Day() -> Element {
    let app = use_app();
    let mut pick = use_signal(|| None::<Pick>);
    let day = app.day.read().clone();
    // Демо: сразу показать детали прерванной сессии.
    if *app.preset.peek() == UiPreset::Day && pick.peek().is_none() {
        if let Some(s) = day.as_ref().and_then(|d| {
            d.sessions
                .iter()
                .find(|s| s.end_reason == Some(EndReason::Interrupted))
        }) {
            pick.set(Some(Pick::Session(s.id)));
        }
    }
    // Шкала прокручена к «сейчас» при открытии экрана.
    let mut scrolled = use_signal(|| false);
    use_effect(move || {
        if app.day.read().is_some() && !*scrolled.peek() {
            scrolled.set(true);
            document::eval(
                "setTimeout(() => { const el = document.querySelector('.timeline-scroll'); const now = document.querySelector('.now-line'); if (el && now) el.scrollTop = Math.max(0, now.offsetTop - el.clientHeight * 0.7); }, 60);",
            );
        }
    });
    let now = app
        .status
        .read()
        .as_ref()
        .map(|s| s.server_time)
        .unwrap_or_else(Utc::now);

    let Some(day) = day else {
        return rsx! { div { class: "page day" } };
    };
    let t = day.totals;
    let interrupted: Vec<&SessionDto> = day
        .sessions
        .iter()
        .filter(|s| s.end_reason == Some(EndReason::Interrupted))
        .collect();
    let interrupted_sec: i64 = interrupted.iter().map(|s| s.duration_sec).sum();

    let Some(info) = day.day.clone() else {
        return rsx! {
            div { class: "page day",
                header { class: "page-head",
                    div {
                        h1 { class: "page-title", "{fmt::long_date(day.date)}" }
                        p { class: "page-sub", "День ещё не начат" }
                    }
                }
                div { class: "empty",
                    h2 { class: "empty-title", "Шкала пуста" }
                    p { class: "empty-text", "День откроется сам, когда начнёшь первую сессию на экране «Фокус»." }
                }
            }
        };
    };

    // Шкала — от начала часа открытия до конца текущего часа.
    let start = info.opened_at;
    let end = info
        .closed_at
        .unwrap_or(now)
        .max(start + Duration::minutes(1));
    let local_start = start.with_timezone(&focus_api_types::chrono::Local);
    let grid_start = start
        - Duration::minutes(i64::from(local_start.minute()))
        - Duration::seconds(i64::from(local_start.second()));
    let span_min = ((end - grid_start).num_minutes() / 60 + 1) * 60;
    let height = span_min as f64 * PX_PER_MIN;
    let y = |t: DateTime<Utc>| (t - grid_start).num_seconds() as f64 / 60.0 * PX_PER_MIN;
    let hours: Vec<(f64, String)> = (0..=span_min / 60)
        .map(|h| {
            let t = grid_start + Duration::hours(h);
            (y(t), fmt::local_hm(t))
        })
        .collect();
    let now_y = info.closed_at.is_none().then(|| y(now));

    let selected = pick();
    let detail = match selected {
        Some(Pick::Session(id)) => day.sessions.iter().find(|s| s.id == id).map(|s| {
            let ended = s.ended_at.unwrap_or(now);
            Detail {
                class: kind_class(s.kind),
                kind: kind_label(s.kind).to_string(),
                item: s.task_title.clone(),
                range: format!("{} – {}", fmt::local_hm(s.started_at), fmt::local_hm(ended)),
                duration: fmt::duration(s.duration_sec),
                reason: reason_label(s.end_reason, s.kind).to_string(),
                interrupted: s.end_reason == Some(EndReason::Interrupted),
            }
        }),
        Some(Pick::Lost(i)) => day.lost.get(i).map(|r: &TimeRange| Detail {
            class: "lost",
            kind: "Потерянное время".into(),
            item: None,
            range: format!("{} – {}", fmt::local_hm(r.start), fmt::local_hm(r.end)),
            duration: fmt::duration((r.end - r.start).num_seconds()),
            reason: "Между сессиями: в норму не идёт".into(),
            interrupted: false,
        }),
        None => None,
    };

    let pct = (t.worked_sec as f64 / t.target_sec.max(1) as f64 * 100.0).round() as i64;

    rsx! {
        div { class: "page day",
            header { class: "page-head",
                div {
                    h1 { class: "page-title", "{fmt::long_date(day.date)}" }
                    p { class: "page-sub",
                        "Начат в "
                        span { class: "num", "{fmt::local_hm(info.opened_at)}" }
                        " · в норму "
                        span { class: "num", "{fmt::hm(t.worked_sec)} из {fmt::hm(t.target_sec)}" }
                        " ({pct}%)"
                    }
                }
            }

            div { class: "totals",
                Total { class: "work", label: "Работа", value: fmt::hm(t.work_sec) }
                Total { class: "planning", label: "Планирование", value: fmt::hm(t.planning_sec) }
                Total { class: "rest", label: "Отдых", value: fmt::hm(t.rest_sec) }
                Total { class: "lost", label: "Потеряно", value: fmt::hm(t.lost_sec) }
                Total {
                    class: "interrupted",
                    label: "Прервано",
                    value: format!("{}", interrupted.len()),
                    note: if interrupted.is_empty() { String::new() } else { fmt::duration(interrupted_sec) },
                }
            }

            div { class: "day-body",
                div { class: "timeline-scroll",
                    div { class: "timeline", style: "height: {height:.0}px",
                        for (i, (top, label)) in hours.iter().enumerate() {
                            div { key: "h{i}", class: "hour", style: "top: {top:.1}px",
                                span { class: "hour-label num", "{label}" }
                            }
                        }
                        for (i, r) in day.lost.iter().enumerate() {
                            button {
                                key: "l{i}",
                                class: if selected == Some(Pick::Lost(i)) { "block lost selected" } else { "block lost" },
                                style: "top: {y(r.start):.1}px; height: {(y(r.end) - y(r.start)).max(2.0):.1}px",
                                "aria-label": "Потерянное время, {fmt::duration((r.end - r.start).num_seconds())}",
                                onclick: move |_| pick.set(Some(Pick::Lost(i))),
                            }
                        }
                        for s in day.sessions.iter() {
                            SessionBlock {
                                key: "{s.id}",
                                session: s.clone(),
                                top: y(s.started_at),
                                height: (y(s.ended_at.unwrap_or(now)) - y(s.started_at)).max(2.0),
                                selected: selected == Some(Pick::Session(s.id)),
                                on_pick: move |id| pick.set(Some(Pick::Session(id))),
                            }
                        }
                        if let Some(ny) = now_y {
                            div { class: "now-line", style: "top: {ny:.1}px",
                                span { class: "now-label num", "{fmt::local_hm(now)}" }
                            }
                        }
                    }
                }
                aside { class: "detail",
                    if let Some(d) = detail {
                        div { class: "detail-card detail-{d.class}",
                            div { class: "detail-kind",
                                span { class: "kind-dot {d.class}" }
                                "{d.kind}"
                            }
                            if let Some(item) = &d.item {
                                p { class: "detail-item", "{item}" }
                            }
                            dl { class: "detail-list",
                                dt { "Время" }
                                dd { class: "num", "{d.range}" }
                                dt { "Длительность" }
                                dd { class: "num", "{d.duration}" }
                                dt { "Итог" }
                                dd { class: if d.interrupted { "reason interrupted" } else { "reason" }, "{d.reason}" }
                            }
                        }
                    } else {
                        div { class: "detail-hint",
                            p { "Нажми на блок шкалы, чтобы увидеть подробности сессии." }
                            ul { class: "legend",
                                li { span { class: "kind-dot work" } "Работа" }
                                li { span { class: "kind-dot planning" } "Планирование" }
                                li { span { class: "kind-dot rest" } "Отдых" }
                                li { span { class: "kind-dot lost" } "Потерянное время" }
                                li { span { class: "kind-dot interrupted" } "Прервана" }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
struct Detail {
    class: &'static str,
    kind: String,
    item: Option<String>,
    range: String,
    duration: String,
    reason: String,
    interrupted: bool,
}

#[component]
fn Total(
    class: &'static str,
    label: &'static str,
    value: String,
    #[props(default)] note: String,
) -> Element {
    rsx! {
        div { class: "total total-{class}",
            div { class: "total-label",
                span { class: "kind-dot {class}" }
                "{label}"
            }
            div { class: "total-value num", "{value}" }
            if !note.is_empty() {
                div { class: "total-note num", "{note}" }
            }
        }
    }
}

#[component]
fn SessionBlock(
    session: SessionDto,
    top: f64,
    height: f64,
    selected: bool,
    on_pick: EventHandler<SessionId>,
) -> Element {
    let s = &session;
    let id = s.id;
    let mut class = format!("block {}", kind_class(s.kind));
    if s.end_reason == Some(EndReason::Interrupted) {
        class.push_str(" interrupted");
    }
    if s.ended_at.is_none() {
        class.push_str(" active");
    }
    if selected {
        class.push_str(" selected");
    }
    if height < 13.0 {
        class.push_str(" tiny");
    } else if height < 22.0 {
        class.push_str(" compact");
    }
    let title = match (s.kind, &s.task_title) {
        (SessionKind::Work, Some(t)) => t.clone(),
        (k, _) => kind_label(k).to_string(),
    };
    let dur = fmt::duration(s.duration_sec);
    rsx! {
        button {
            class,
            style: "top: {top:.1}px; height: {height:.1}px",
            "aria-label": "{kind_label(s.kind)}: {title}, {dur}",
            onclick: move |_| on_pick.call(id),
            span { class: "block-title", "{title}" }
            span { class: "block-dur num", "{dur}" }
        }
    }
}
