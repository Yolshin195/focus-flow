//! Что показывает таймер — одно место для экрана «Фокус» и индикаторов вне
//! окна (трей / строка меню на desktop).
//!
//! Своего хода времени здесь нет: всё считается из досчитанного статуса
//! [`AppState::status`](crate::state::AppState) — от `ends_at` / `started_at`
//! и `server_time` последнего снимка сервиса.

use dioxus::prelude::*;
use focus_api_types::{ActiveSession, ItemRef, StatusDto};

use crate::fmt;
use crate::state::{use_app, Conn};

/// Фаза таймера и время в ней.
#[derive(Debug, Clone, PartialEq)]
pub enum TimerView {
    /// Данных от сервиса ещё нет.
    Loading,
    /// Ничего не идёт.
    Idle,
    /// Время работы вышло, отдых ждёт «Начать отдых»; прошло с конца работы
    /// (секунды, потерянное время).
    RestDue {
        /// Прошло с конца работы, с.
        waiting: i64,
    },
    /// Работа: осталось и всего (секунды), пункт.
    Work {
        /// Осталось, с (не меньше нуля).
        remaining: i64,
        /// Длина сессии, с (не меньше единицы).
        total: i64,
        /// Пункт в работе.
        item: ItemRef,
    },
    /// Отдых: осталось и всего (секунды).
    Rest {
        /// Осталось, с (не меньше нуля).
        remaining: i64,
        /// Длина отдыха, с (не меньше единицы).
        total: i64,
    },
    /// Планирование: прошло (секунды).
    Planning {
        /// Прошло, с (не меньше нуля).
        elapsed: i64,
    },
}

impl TimerView {
    /// Вид по статусу (уже досчитанному до «сейчас»).
    pub fn of(status: Option<&StatusDto>) -> Self {
        let Some(s) = status else {
            return Self::Loading;
        };
        let now = s.server_time;
        match &s.active {
            None if s.rest_due => Self::RestDue {
                waiting: s.idle_since.map_or(0, |at| (now - at).num_seconds().max(0)),
            },
            None => Self::Idle,
            Some(ActiveSession::Work {
                item,
                started_at,
                ends_at,
                ..
            }) => Self::Work {
                remaining: (*ends_at - now).num_seconds().max(0),
                total: (*ends_at - *started_at).num_seconds().max(1),
                item: item.clone(),
            },
            Some(ActiveSession::Rest {
                started_at,
                ends_at,
                ..
            }) => Self::Rest {
                remaining: (*ends_at - now).num_seconds().max(0),
                total: (*ends_at - *started_at).num_seconds().max(1),
            },
            Some(ActiveSession::Planning { started_at, .. }) => Self::Planning {
                elapsed: (now - *started_at).num_seconds().max(0),
            },
        }
    }

    /// Время на циферблате: остаток у работы и отдыха, прошедшее у
    /// планирования (`49:55`, `1:02:05`). В простое и до данных — нет.
    pub fn clock(&self) -> Option<String> {
        match self {
            Self::Work { remaining, .. } | Self::Rest { remaining, .. } => {
                Some(fmt::clock(*remaining))
            }
            Self::Planning { elapsed } => Some(fmt::clock(*elapsed)),
            Self::RestDue { .. } => Some(fmt::clock(0)),
            Self::Idle | Self::Loading => None,
        }
    }

    /// Пункт в работе.
    pub fn item(&self) -> Option<&ItemRef> {
        match self {
            Self::Work { item, .. } => Some(item),
            _ => None,
        }
    }
}

/// Вид таймера из состояния приложения. Пока идёт сессия, меняется раз в
/// секунду — вместе с досчитанным статусом.
pub fn use_timer_view() -> Memo<TimerView> {
    let app = use_app();
    use_memo(move || TimerView::of(app.status.read().as_ref()))
}

/// Состояние для индикатора вне окна (иконка в трее / строке меню).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Indicator {
    /// Идёт работа.
    Work,
    /// Идёт отдых.
    Rest,
    /// Идёт планирование.
    Planning,
    /// Ничего не идёт.
    Idle,
    /// Время работы вышло — ждём «Начать отдых».
    RestDue,
    /// Нет связи с сервисом (или данных ещё нет).
    Offline,
}

/// Строка состояния вне окна: иконка, текст рядом с ней, подсказка.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusLine {
    /// Какую иконку показать.
    pub indicator: Indicator,
    /// Текст рядом с иконкой: время работы, отдыха или планирования.
    /// В простое и без связи — нет.
    pub title: Option<String>,
    /// Подсказка при наведении: фаза, время, пункт.
    pub tooltip: String,
}

/// Подсказка в трее Windows — до 127 символов.
const TOOLTIP_MAX: usize = 127;
/// Сколько символов названия пункта влезает в подсказку.
const TOOLTIP_ITEM_MAX: usize = 60;

impl StatusLine {
    /// Строка по виду таймера и состоянию связи. Без связи последние данные
    /// не показываются: время могло уйти, а команды не дойдут.
    pub fn of(view: &TimerView, conn: &Conn) -> Self {
        let offline = |tooltip: &str| Self {
            indicator: Indicator::Offline,
            title: None,
            tooltip: tooltip.into(),
        };
        match conn {
            Conn::Offline => return offline("Focus Flow — нет связи с сервисом"),
            Conn::Unauthorized => return offline("Focus Flow — сервис не принял токен"),
            Conn::Unknown | Conn::Online => {}
        }
        let title = view.clock();
        let time = title.clone().unwrap_or_default();
        let (indicator, tooltip) = match view {
            TimerView::Loading => return offline("Focus Flow — подключаюсь к сервису"),
            TimerView::Idle => (Indicator::Idle, "Focus Flow — готов к работе".to_string()),
            TimerView::RestDue { .. } => (
                Indicator::RestDue,
                "Время работы вышло — начни отдых".to_string(),
            ),
            TimerView::Work { item, .. } => (
                Indicator::Work,
                format!(
                    "Работа: осталось {time} — {}",
                    truncate(&item.title, TOOLTIP_ITEM_MAX)
                ),
            ),
            TimerView::Rest { .. } => (Indicator::Rest, format!("Отдых: осталось {time}")),
            TimerView::Planning { .. } => {
                (Indicator::Planning, format!("Планирование: идёт {time}"))
            }
        };
        Self {
            indicator,
            title,
            tooltip: truncate(&tooltip, TOOLTIP_MAX),
        }
    }
}

/// Строка состояния из состояния приложения: тот же [`use_timer_view`] плюс связь.
pub fn use_status_line() -> Memo<StatusLine> {
    let app = use_app();
    let view = use_timer_view();
    use_memo(move || StatusLine::of(&view.read(), &app.conn.read()))
}

/// Обрезать до `max` символов (не байт) с многоточием.
fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_owned();
    }
    let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use focus_api_types::chrono::{DateTime, Duration, Utc};
    use focus_api_types::{SessionId, TaskId, Uuid};

    fn t0() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-10-24T09:00:00Z")
            .map(|t| t.with_timezone(&Utc))
            .unwrap()
    }

    fn status(active: Option<ActiveSession>, now: DateTime<Utc>) -> StatusDto {
        StatusDto {
            server_time: now,
            day: None,
            active,
            worked_today_sec: 0,
            target_sec: 8 * 3600,
            idle_since: None,
            rest_due: false,
        }
    }

    fn item(title: &str) -> ItemRef {
        ItemRef {
            id: TaskId::from(Uuid::nil()),
            title: title.into(),
            parent_id: None,
            parent_title: None,
        }
    }

    fn work(elapsed: Duration) -> StatusDto {
        let start = t0();
        status(
            Some(ActiveSession::Work {
                session_id: SessionId::from(Uuid::nil()),
                item: item("Написать тесты"),
                started_at: start,
                ends_at: start + Duration::minutes(50),
            }),
            start + elapsed,
        )
    }

    fn rest(elapsed: Duration) -> StatusDto {
        let start = t0();
        status(
            Some(ActiveSession::Rest {
                session_id: SessionId::from(Uuid::nil()),
                started_at: start,
                ends_at: start + Duration::minutes(10),
            }),
            start + elapsed,
        )
    }

    fn planning(elapsed: Duration) -> StatusDto {
        let start = t0();
        status(
            Some(ActiveSession::Planning {
                session_id: SessionId::from(Uuid::nil()),
                started_at: start,
            }),
            start + elapsed,
        )
    }

    fn line(s: &StatusDto) -> StatusLine {
        StatusLine::of(&TimerView::of(Some(s)), &Conn::Online)
    }

    #[test]
    fn clock_boundaries() {
        assert_eq!(fmt::clock(0), "00:00");
        assert_eq!(fmt::clock(59), "00:59");
        assert_eq!(fmt::clock(59 * 60 + 59), "59:59");
        assert_eq!(fmt::clock(3600), "1:00:00");
        assert_eq!(fmt::clock(3600 + 2 * 60 + 5), "1:02:05");
        assert_eq!(fmt::clock(10 * 3600), "10:00:00");
        assert_eq!(fmt::clock(-1), "00:00");
        assert_eq!(fmt::clock(i64::MIN), "00:00");
    }

    #[test]
    fn work_counts_down() {
        let l = line(&work(Duration::seconds(5)));
        assert_eq!(l.indicator, Indicator::Work);
        assert_eq!(l.title.as_deref(), Some("49:55"));
        assert_eq!(l.tooltip, "Работа: осталось 49:55 — Написать тесты");
        assert_eq!(
            line(&work(Duration::zero())).title.as_deref(),
            Some("50:00")
        );
    }

    #[test]
    fn overdue_is_zero() {
        // Сервис ещё не закрыл сессию по лимиту, а время уже вышло.
        let v = TimerView::of(Some(&work(Duration::minutes(51))));
        assert!(matches!(v, TimerView::Work { remaining: 0, .. }));
        assert_eq!(v.clock().as_deref(), Some("00:00"));
        let v = TimerView::of(Some(&rest(Duration::minutes(12))));
        assert_eq!(v.clock().as_deref(), Some("00:00"));
    }

    #[test]
    fn rest_counts_down() {
        let l = line(&rest(Duration::seconds(48)));
        assert_eq!(l.indicator, Indicator::Rest);
        assert_eq!(l.title.as_deref(), Some("09:12"));
        assert_eq!(l.tooltip, "Отдых: осталось 09:12");
    }

    #[test]
    fn planning_counts_up() {
        let l = line(&planning(Duration::seconds(12 * 60 + 40)));
        assert_eq!(l.indicator, Indicator::Planning);
        assert_eq!(l.title.as_deref(), Some("12:40"));
        assert_eq!(
            line(&planning(Duration::seconds(59 * 60 + 59)))
                .title
                .as_deref(),
            Some("59:59")
        );
        assert_eq!(
            line(&planning(Duration::minutes(62) + Duration::seconds(5)))
                .title
                .as_deref(),
            Some("1:02:05")
        );
        // Часы клиента чуть отстали от начала — не уходим в минус.
        assert_eq!(
            line(&planning(Duration::seconds(-3))).title.as_deref(),
            Some("00:00")
        );
    }

    #[test]
    fn idle_has_no_text() {
        let l = line(&status(None, t0()));
        assert_eq!(l.indicator, Indicator::Idle);
        assert_eq!(l.title, None);
    }

    #[test]
    fn offline_hides_last_data() {
        let v = TimerView::of(Some(&work(Duration::seconds(5))));
        for conn in [Conn::Offline, Conn::Unauthorized] {
            let l = StatusLine::of(&v, &conn);
            assert_eq!(l.indicator, Indicator::Offline);
            assert_eq!(l.title, None);
        }
        // Данных ещё нет — тоже «нет связи».
        let l = StatusLine::of(&TimerView::Loading, &Conn::Unknown);
        assert_eq!(l.indicator, Indicator::Offline);
        assert_eq!(l.title, None);
    }

    #[test]
    fn long_item_is_truncated() {
        let mut s = work(Duration::zero());
        if let Some(ActiveSession::Work { item, .. }) = &mut s.active {
            item.title = "я".repeat(300);
        }
        let l = line(&s);
        assert!(l.tooltip.chars().count() <= TOOLTIP_MAX);
        assert!(l.tooltip.ends_with('…'));
        assert_eq!(truncate("коротко", 10), "коротко");
        assert_eq!(truncate("абвгд", 3), "аб…");
    }
}
