//! Чистые правила без портов: потерянное время, итоги дня, истечение лимита,
//! дерево задач. Всё, что можно проверить без хранилища, — здесь.

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use focus_api_types::{DayTotalsDto, SessionKind, TaskDto, TaskId, TimeRange};

use crate::domain::{Session, Task, WorkDay};

/// Если лимит активной сессии истёк к `now`, — момент, в который она должна
/// была закончиться (`ends_at`). Планирование не истекает никогда.
pub fn expired_at(session: &Session, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
    let ends_at = session.ends_at()?;
    (session.is_active() && now >= ends_at).then_some(ends_at)
}

/// Потерянное время дня: промежутки без сессии внутри
/// `[opened_at, until)`. `until` — `closed_at` закрытого дня, «сейчас» для
/// сегодняшнего открытого дня; `None` — хвост после последней сессии не
/// считается (прошлый незакрытый день).
///
/// `sessions` — сессии этого дня в порядке начала.
pub fn lost_ranges(
    day: &WorkDay,
    sessions: &[Session],
    now: DateTime<Utc>,
    until: Option<DateTime<Utc>>,
) -> Vec<TimeRange> {
    let mut lost = Vec::new();
    let mut cursor = day.opened_at;
    for s in sessions {
        if s.started_at > cursor {
            lost.push(TimeRange {
                start: cursor,
                end: s.started_at,
            });
        }
        cursor = cursor.max(s.end_or(now));
    }
    if let Some(until) = until
        && until > cursor
    {
        lost.push(TimeRange {
            start: cursor,
            end: until,
        });
    }
    lost
}

/// Начало текущего потерянного времени: конец последней сессии дня (или
/// открытие дня, если сессий не было). `None`, если идёт сессия.
pub fn idle_since(
    day: &WorkDay,
    sessions: &[Session],
    now: DateTime<Utc>,
) -> Option<DateTime<Utc>> {
    if sessions.iter().any(Session::is_active) {
        return None;
    }
    let last_end = sessions.iter().map(|s| s.end_or(now)).max();
    Some(last_end.map_or(day.opened_at, |e| e.max(day.opened_at)))
}

/// Отдых положен, но не начат: последняя сессия дня — работа, закрытая
/// по лимиту. `sessions` — сессии этого дня.
pub fn rest_due(sessions: &[Session]) -> bool {
    sessions
        .iter()
        .max_by_key(|s| s.started_at)
        .is_some_and(|s| {
            s.kind == SessionKind::Work
                && !s.is_active()
                && s.end_reason == Some(focus_api_types::EndReason::Limit)
        })
}

/// Сумма длительностей сессий вида `kind` на момент `now`.
pub fn sum_kind(sessions: &[Session], kind: SessionKind, now: DateTime<Utc>) -> i64 {
    sessions
        .iter()
        .filter(|s| s.kind == kind)
        .map(|s| s.duration_sec(now))
        .sum()
}

/// В норму: `work` + `planning`.
pub fn worked_sec(sessions: &[Session], now: DateTime<Utc>) -> i64 {
    sessions
        .iter()
        .filter(|s| s.counts_to_norm())
        .map(|s| s.duration_sec(now))
        .sum()
}

/// Итоги дня.
pub fn totals(
    sessions: &[Session],
    lost: &[TimeRange],
    now: DateTime<Utc>,
    target_sec: i64,
) -> DayTotalsDto {
    let work_sec = sum_kind(sessions, SessionKind::Work, now);
    let planning_sec = sum_kind(sessions, SessionKind::Planning, now);
    DayTotalsDto {
        work_sec,
        planning_sec,
        rest_sec: sum_kind(sessions, SessionKind::Rest, now),
        lost_sec: lost.iter().map(|r| (r.end - r.start).num_seconds()).sum(),
        worked_sec: work_sec + planning_sec,
        target_sec,
    }
}

fn sort_key(t: &Task) -> (i64, DateTime<Utc>, TaskId) {
    (t.sort_order, t.created_at, t.id)
}

/// Строит дерево из плоского списка. Корни — узлы без родителя и узлы,
/// чей родитель отсутствует в списке. Соседи — по `(sort_order, created_at, id)`.
pub fn build_tree(tasks: &[Task]) -> Vec<TaskDto> {
    let ids: std::collections::BTreeSet<TaskId> = tasks.iter().map(|t| t.id).collect();
    let mut by_parent: BTreeMap<Option<TaskId>, Vec<&Task>> = BTreeMap::new();
    for t in tasks {
        let parent = t.parent_id.filter(|p| ids.contains(p));
        by_parent.entry(parent).or_default().push(t);
    }
    for v in by_parent.values_mut() {
        v.sort_by_key(|t| sort_key(t));
    }
    fn build(t: &Task, by_parent: &BTreeMap<Option<TaskId>, Vec<&Task>>, depth: u8) -> TaskDto {
        let mut dto = t.to_dto();
        // Глубина ограничена при создании; предохранитель от циклов в данных.
        if depth < 8 {
            dto.children = by_parent
                .get(&Some(t.id))
                .map(|cs| cs.iter().map(|c| build(c, by_parent, depth + 1)).collect())
                .unwrap_or_default();
        }
        dto
    }
    by_parent
        .get(&None)
        .map(|roots| roots.iter().map(|t| build(t, &by_parent, 0)).collect())
        .unwrap_or_default()
}

/// Поддерево узла `id` из плоского списка.
pub fn subtree(tasks: &[Task], id: TaskId) -> Option<TaskDto> {
    build_tree(tasks).iter().find_map(|t| t.find(id)).cloned()
}

#[cfg(test)]
mod tests {
    use chrono::{Duration, NaiveDate, TimeZone};
    use focus_api_types::{DayId, EndReason, SessionId, TaskStatus, Uuid};

    use super::*;

    fn t(min: i64) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, 24, 9, 0, 0).unwrap() + Duration::minutes(min)
    }

    fn day() -> WorkDay {
        WorkDay {
            id: DayId::from(Uuid::from_u128(1)),
            date: NaiveDate::from_ymd_opt(2026, 10, 24).unwrap(),
            opened_at: t(0),
            closed_at: None,
            target_sec: 8 * 3600,
        }
    }

    fn session(n: u128, kind: SessionKind, start: i64, end: Option<i64>) -> Session {
        Session {
            id: SessionId::from(Uuid::from_u128(n)),
            day_id: day().id,
            kind,
            task_id: None,
            started_at: t(start),
            ended_at: end.map(t),
            limit_sec: match kind {
                SessionKind::Work => Some(3000),
                SessionKind::Rest => Some(600),
                SessionKind::Planning => None,
            },
            end_reason: end.map(|_| EndReason::Limit),
        }
    }

    #[test]
    fn work_expires_at_limit_planning_never() {
        let work = session(1, SessionKind::Work, 0, None);
        assert_eq!(expired_at(&work, t(49)), None);
        assert_eq!(expired_at(&work, t(50)), Some(t(50)));
        assert_eq!(expired_at(&work, t(90)), Some(t(50)));
        let planning = session(2, SessionKind::Planning, 0, None);
        assert_eq!(expired_at(&planning, t(1000)), None);
        let closed = session(3, SessionKind::Rest, 0, Some(5));
        assert_eq!(expired_at(&closed, t(1000)), None);
    }

    #[test]
    fn lost_time_is_gaps_between_sessions_and_tail() {
        // 9:05–9:55 работа, 9:55–10:05 отдых, потом тишина до 10:20, планирование 10:20–10:30.
        let sessions = vec![
            session(1, SessionKind::Work, 5, Some(55)),
            session(2, SessionKind::Rest, 55, Some(65)),
            session(3, SessionKind::Planning, 80, Some(90)),
        ];
        let lost = lost_ranges(&day(), &sessions, t(100), Some(t(100)));
        assert_eq!(
            lost,
            vec![
                TimeRange {
                    start: t(0),
                    end: t(5)
                },
                TimeRange {
                    start: t(65),
                    end: t(80)
                },
                TimeRange {
                    start: t(90),
                    end: t(100)
                },
            ]
        );
        let tot = totals(&sessions, &lost, t(100), 8 * 3600);
        assert_eq!(tot.work_sec, 50 * 60);
        assert_eq!(tot.rest_sec, 10 * 60);
        assert_eq!(tot.planning_sec, 10 * 60);
        assert_eq!(tot.worked_sec, 60 * 60);
        assert_eq!(tot.lost_sec, 30 * 60);
        assert_eq!(idle_since(&day(), &sessions, t(100)), Some(t(90)));
        // Прошлый незакрытый день — хвост не считается.
        assert_eq!(lost_ranges(&day(), &sessions, t(100), None).len(), 2);
    }

    #[test]
    fn active_session_has_no_lost_tail_and_counts_till_now() {
        let sessions = vec![session(1, SessionKind::Work, 0, None)];
        let lost = lost_ranges(&day(), &sessions, t(20), Some(t(20)));
        assert!(lost.is_empty());
        assert_eq!(worked_sec(&sessions, t(20)), 20 * 60);
        assert_eq!(idle_since(&day(), &sessions, t(20)), None);
    }

    #[test]
    fn rest_is_not_in_norm() {
        let sessions = vec![
            session(1, SessionKind::Rest, 0, Some(10)),
            session(2, SessionKind::Planning, 10, Some(15)),
        ];
        assert_eq!(worked_sec(&sessions, t(30)), 5 * 60);
    }

    #[test]
    fn tree_orders_children_and_nests() {
        let mk = |n: u128, parent: Option<u128>, order: i64| Task {
            id: TaskId::from(Uuid::from_u128(n)),
            parent_id: parent.map(|p| TaskId::from(Uuid::from_u128(p))),
            title: format!("t{n}"),
            notes: None,
            status: TaskStatus::Planned,
            planned_for: None,
            estimate_intervals: None,
            sort_order: order,
            created_at: t(0),
            done_at: None,
            external_url: None,
        };
        let tasks = vec![
            mk(3, Some(1), 2),
            mk(1, None, 0),
            mk(2, Some(1), 1),
            mk(4, None, -1),
        ];
        let tree = build_tree(&tasks);
        assert_eq!(tree.len(), 2);
        assert_eq!(tree[0].title, "t4");
        let kids: Vec<_> = tree[1].children.iter().map(|c| c.title.as_str()).collect();
        assert_eq!(kids, ["t2", "t3"]);
        let sub = subtree(&tasks, TaskId::from(Uuid::from_u128(1))).unwrap();
        assert_eq!(sub.children.len(), 2);
    }
}
