//! Интеграционные тесты хранилища на временной БД: репозитории, дерево,
//! outbox, переоткрытие файла и все сценарии контракта поверх SQLite.

use std::sync::Arc;

use chrono::{DateTime, Duration, NaiveDate, TimeZone, Utc};
use focus_api_contract_tests::{TestClock, assert_all_passed, run_all_with_clock};
use focus_api_types::{
    DayId, EndReason, NewTask, SessionId, SessionKind, TaskId, TaskPatch, TaskStatus, Uuid,
};
use focus_core::{
    DayRepo, DomainEvent, EventSink, FocusService, ManualClock, NoTaskSources, Outbox, Session,
    SessionRepo, Settings, SettingsRepo, Task, TaskRepo, WorkDay,
};
use focus_storage::Storage;

/// Подписчики outbox в тесте очереди.
fn ab() -> Vec<String> {
    vec!["a".into(), "b".into()]
}

fn t0() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 10, 24, 9, 0, 0).unwrap()
}

fn day(date: NaiveDate) -> WorkDay {
    WorkDay {
        id: DayId::from(Uuid::now_v7()),
        date,
        opened_at: t0(),
        closed_at: None,
        target_sec: 8 * 3600,
    }
}

fn task(parent: Option<TaskId>, title: &str, sort_order: i64, created: DateTime<Utc>) -> Task {
    Task {
        id: TaskId::from(Uuid::now_v7()),
        parent_id: parent,
        title: title.into(),
        notes: None,
        status: TaskStatus::Planned,
        planned_for: None,
        estimate_intervals: None,
        sort_order,
        created_at: created,
        done_at: None,
        external_url: None,
    }
}

fn session(
    day_id: DayId,
    kind: SessionKind,
    task_id: Option<TaskId>,
    at: DateTime<Utc>,
) -> Session {
    Session {
        id: SessionId::from(Uuid::now_v7()),
        day_id,
        kind,
        task_id,
        started_at: at,
        ended_at: None,
        limit_sec: (kind != SessionKind::Planning).then_some(3000),
        end_reason: None,
    }
}

async fn mem() -> Storage {
    Storage::open_in_memory().await.unwrap()
}

#[tokio::test]
async fn days_crud() {
    let st = mem().await;
    let date = NaiveDate::from_ymd_opt(2026, 10, 24).unwrap();
    assert_eq!(
        DayRepo::get(&st, DayId::from(Uuid::now_v7()))
            .await
            .unwrap(),
        None
    );
    assert_eq!(st.by_date(date).await.unwrap(), None);

    let mut d = day(date);
    DayRepo::save(&st, &d).await.unwrap();
    assert_eq!(DayRepo::get(&st, d.id).await.unwrap(), Some(d.clone()));
    assert_eq!(st.by_date(date).await.unwrap(), Some(d.clone()));

    d.closed_at = Some(t0() + Duration::hours(9) + Duration::nanoseconds(123));
    DayRepo::save(&st, &d).await.unwrap();
    assert_eq!(st.by_date(date).await.unwrap(), Some(d.clone()));

    // Второй день на ту же дату — нарушение уникальности.
    assert!(DayRepo::save(&st, &day(date)).await.is_err());
}

#[tokio::test]
async fn tasks_crud_and_tree_order() {
    let st = mem().await;
    let root = task(None, "задача", 2, t0());
    let other = task(None, "раньше", 1, t0() + Duration::minutes(5));
    TaskRepo::save(&st, &root).await.unwrap();
    TaskRepo::save(&st, &other).await.unwrap();

    // Пункты: порядок (sort_order, created_at, id).
    let c3 = task(Some(root.id), "в", 2, t0());
    let c1 = task(Some(root.id), "а", 1, t0() + Duration::minutes(2));
    let c2 = task(Some(root.id), "б", 1, t0() + Duration::minutes(3));
    for c in [&c3, &c1, &c2] {
        TaskRepo::save(&st, c).await.unwrap();
    }

    let top: Vec<_> = st
        .children(None)
        .await
        .unwrap()
        .into_iter()
        .map(|t| t.id)
        .collect();
    assert_eq!(top, [other.id, root.id]);
    let kids: Vec<_> = st
        .children(Some(root.id))
        .await
        .unwrap()
        .into_iter()
        .map(|t| t.title)
        .collect();
    assert_eq!(kids, ["а", "б", "в"]);
    assert_eq!(st.list().await.unwrap().len(), 5);
    assert!(st.children(Some(c1.id)).await.unwrap().is_empty());

    // Правка всех полей.
    let mut e = c1.clone();
    e.title = "а'".into();
    e.notes = Some("заметка".into());
    e.status = TaskStatus::Done;
    e.planned_for = NaiveDate::from_ymd_opt(2026, 10, 25);
    e.estimate_intervals = Some(3);
    e.done_at = Some(t0() + Duration::hours(1));
    e.external_url = Some("https://jira/browse/X-1".into());
    TaskRepo::save(&st, &e).await.unwrap();
    assert_eq!(TaskRepo::get(&st, e.id).await.unwrap(), Some(e.clone()));
    assert_eq!(
        st.by_external_url("https://jira/browse/X-1").await.unwrap(),
        Some(e.clone())
    );
    assert_eq!(st.by_external_url("нет").await.unwrap(), None);

    // Родитель должен существовать (внешний ключ).
    let orphan = task(Some(TaskId::from(Uuid::now_v7())), "сирота", 1, t0());
    assert!(TaskRepo::save(&st, &orphan).await.is_err());
}

#[tokio::test]
async fn sessions_crud_and_single_active() {
    let st = mem().await;
    let d = day(NaiveDate::from_ymd_opt(2026, 10, 24).unwrap());
    DayRepo::save(&st, &d).await.unwrap();
    let item = task(None, "пункт", 1, t0());
    TaskRepo::save(&st, &item).await.unwrap();

    assert_eq!(st.active().await.unwrap(), None);
    let mut w = session(d.id, SessionKind::Work, Some(item.id), t0());
    SessionRepo::save(&st, &w).await.unwrap();
    assert_eq!(st.active().await.unwrap(), Some(w.clone()));

    // Вторая активная — запрещена индексом.
    let p = session(
        d.id,
        SessionKind::Planning,
        None,
        t0() + Duration::minutes(1),
    );
    assert!(SessionRepo::save(&st, &p).await.is_err());

    w.ended_at = Some(t0() + Duration::minutes(50));
    w.end_reason = Some(EndReason::Limit);
    SessionRepo::save(&st, &w).await.unwrap();
    assert_eq!(st.active().await.unwrap(), None);
    assert_eq!(SessionRepo::get(&st, w.id).await.unwrap(), Some(w.clone()));

    let r = session(d.id, SessionKind::Rest, None, t0() + Duration::minutes(50));
    let early = {
        let mut s = session(
            d.id,
            SessionKind::Planning,
            None,
            t0() - Duration::minutes(10),
        );
        s.ended_at = Some(t0());
        s.end_reason = Some(EndReason::Switched);
        s
    };
    SessionRepo::save(&st, &r).await.unwrap();
    SessionRepo::save(&st, &early).await.unwrap();
    let ids: Vec<_> = st
        .by_day(d.id)
        .await
        .unwrap()
        .into_iter()
        .map(|s| s.id)
        .collect();
    assert_eq!(ids, [early.id, w.id, r.id]);
    assert!(
        st.by_day(DayId::from(Uuid::now_v7()))
            .await
            .unwrap()
            .is_empty()
    );

    // Работа без пункта — нарушение CHECK.
    let mut bad = session(d.id, SessionKind::Work, None, t0() + Duration::hours(2));
    bad.ended_at = Some(t0() + Duration::hours(3));
    bad.end_reason = Some(EndReason::Interrupted);
    assert!(SessionRepo::save(&st, &bad).await.is_err());
}

#[tokio::test]
async fn settings_default_and_roundtrip() {
    let st = mem().await;
    assert_eq!(SettingsRepo::load(&st).await.unwrap(), Settings::default());
    let mut s = Settings {
        work_min: 45,
        rest_min: 15,
        daily_target_min: 360,
        ..Settings::default()
    };
    s.plugins.insert(
        "jira".into(),
        serde_json::json!({"base_url": "https://j", "mode": "dc"}),
    );
    SettingsRepo::save(&st, &s).await.unwrap();
    assert_eq!(SettingsRepo::load(&st).await.unwrap(), s);
    s.work_min = 50;
    s.plugins.clear();
    SettingsRepo::save(&st, &s).await.unwrap();
    assert_eq!(SettingsRepo::load(&st).await.unwrap(), s);
}

#[tokio::test]
async fn outbox_lifecycle() {
    let st = mem().await;
    let now = t0();
    let ev = DomainEvent::ItemCreated {
        task: task(None, "x", 1, now),
        parent: None,
    };

    // Без подписчиков событие никуда не пишется.
    st.publish(std::slice::from_ref(&ev)).await.unwrap();
    assert!(st.fetch_due(now, &ab(), 10).await.unwrap().is_empty());
    assert!(st.stats().await.unwrap().is_empty());

    st.set_subscribers(&["a".into(), "b".into()]).await.unwrap();
    st.publish(&[ev.clone(), DomainEvent::Unknown])
        .await
        .unwrap();
    let due = st.fetch_due(now, &ab(), 10).await.unwrap();
    let got: Vec<_> = due
        .iter()
        .map(|e| (e.plugin_id.as_str(), e.event.name()))
        .collect();
    assert_eq!(
        got,
        [
            ("a", "item_created"),
            ("b", "item_created"),
            ("a", "unknown"),
            ("b", "unknown")
        ]
    );
    assert_eq!(due[0].event, ev);
    assert_eq!(st.fetch_due(now, &ab(), 3).await.unwrap().len(), 3);

    // Неудача у «a»: задержка 10 с, потом 20 с; очередь «a» ждёт по порядку.
    let a1 = due[0].id;
    let next = st.mark_failed(a1, "сеть", now).await.unwrap();
    assert_eq!(next, now + Duration::seconds(10));
    let ids: Vec<_> = st
        .fetch_due(now, &ab(), 10)
        .await
        .unwrap()
        .iter()
        .map(|e| e.id)
        .collect();
    assert_eq!(ids, [due[1].id, due[3].id]);

    let stats = st.stats().await.unwrap();
    assert_eq!(stats.len(), 2);
    assert_eq!((stats[0].plugin_id.as_str(), stats[0].pending), ("a", 2));
    assert_eq!(stats[0].last_error.as_deref(), Some("сеть"));
    assert_eq!(stats[0].last_error_at, Some(now));
    assert_eq!(stats[1].last_error, None);

    let retry = st.fetch_due(next, &ab(), 10).await.unwrap();
    assert_eq!(retry[0].id, a1);
    assert_eq!(retry[0].attempts, 1);
    assert_eq!(retry[0].last_error.as_deref(), Some("сеть"));
    let next2 = st.mark_failed(a1, "снова", next).await.unwrap();
    assert_eq!(next2, next + Duration::seconds(20));

    // Доставка всего.
    for e in st.fetch_due(next2, &ab(), 10).await.unwrap() {
        st.mark_done(e.id, next2).await.unwrap();
    }
    assert!(
        st.fetch_due(next2 + Duration::days(1), &ab(), 10)
            .await
            .unwrap()
            .is_empty()
    );
    let stats = st.stats().await.unwrap();
    assert!(
        stats
            .iter()
            .all(|s| s.pending == 0 && s.last_error.is_none())
    );

    assert!(st.mark_done(9999, now).await.is_err());
    assert!(st.mark_failed(9999, "x", now).await.is_err());
}

#[tokio::test]
async fn data_survives_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("nested").join("focus.db");
    let date = NaiveDate::from_ymd_opt(2026, 10, 24).unwrap();
    let d = day(date);
    let t = task(None, "сохранится", 1, t0());
    {
        let st = Storage::open(&path).await.unwrap();
        DayRepo::save(&st, &d).await.unwrap();
        TaskRepo::save(&st, &t).await.unwrap();
        st.set_subscribers(&["jira".into()]).await.unwrap();
        st.publish(&[DomainEvent::Unknown]).await.unwrap();
        let journal: String = sqlx::query_scalar("PRAGMA journal_mode")
            .fetch_one(st.pool())
            .await
            .unwrap();
        assert_eq!(journal, "wal");
        let fk: i64 = sqlx::query_scalar("PRAGMA foreign_keys")
            .fetch_one(st.pool())
            .await
            .unwrap();
        assert_eq!(fk, 1);
        st.close().await;
    }
    assert!(path.exists());
    let st = Storage::open(&path).await.unwrap();
    assert_eq!(st.by_date(date).await.unwrap(), Some(d));
    assert_eq!(TaskRepo::get(&st, t.id).await.unwrap(), Some(t));
    assert_eq!(st.stats().await.unwrap()[0].pending, 1);
}

struct Clock(Arc<ManualClock>);
impl TestClock for Clock {
    fn advance(&self, by: Duration) {
        self.0.advance(by);
    }
}

#[tokio::test]
async fn contract_scenarios_pass_on_sqlite() {
    let st = mem().await;
    st.set_subscribers(&["test".into()]).await.unwrap();
    let clock = Arc::new(ManualClock::new(t0()));
    let svc = FocusService::new(st.ports(clock.clone(), Arc::new(NoTaskSources)));
    let results = run_all_with_clock(&svc, &Clock(clock)).await;
    assert_eq!(results.len(), 10);
    assert_all_passed(&results).unwrap_or_else(|e| panic!("{e}"));
    // Сервис публиковал события в outbox SQLite.
    assert!(st.stats().await.unwrap()[0].pending > 0);
}

#[tokio::test]
async fn service_on_file_db_survives_restart() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("focus.db");
    let clock = Arc::new(ManualClock::new(t0()));
    let item_id;
    {
        let st = Storage::open(&path).await.unwrap();
        let svc = FocusService::new(st.ports(clock.clone(), Arc::new(NoTaskSources)));
        let root = svc.create_task(NewTask::new("задача")).await.unwrap();
        let item = svc
            .create_task(NewTask::child(root.id, "пункт"))
            .await
            .unwrap();
        item_id = item.id;
        svc.update_task(
            item.id,
            TaskPatch {
                notes: Some(Some("заметка".into())),
                ..TaskPatch::default()
            },
        )
        .await
        .unwrap();
        svc.start_work(item.id).await.unwrap();
        st.close().await;
    }
    clock.advance(Duration::minutes(20));
    let st = Storage::open(&path).await.unwrap();
    let svc = FocusService::new(st.ports(clock.clone(), Arc::new(NoTaskSources)));
    let status = svc.status().await.unwrap();
    let active = status
        .active
        .expect("работа продолжается после перезапуска");
    assert_eq!(active.kind(), SessionKind::Work);
    assert_eq!(active.started_at(), t0());
    let tree = svc.list_tasks().await.unwrap();
    let item = tree[0].find(item_id).unwrap();
    assert_eq!(item.status, TaskStatus::InProgress);
    assert_eq!(item.notes.as_deref(), Some("заметка"));
}
