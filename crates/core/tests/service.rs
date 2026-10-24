//! Правила таймера на `FocusService` + репозиториях в памяти с фейковыми часами,
//! и все сценарии контракта `focus-api-contract-tests`.

use std::sync::Arc;

use chrono::{DateTime, Duration, FixedOffset, NaiveDate, TimeZone, Utc};
use focus_api_contract_tests::{TestClock, assert_all_passed, run_all_with_clock};
use focus_api_types::{
    ActiveSession, DoneNext, EndReason, NewTask, ServerEvent, SessionKind, SettingsDto, TaskDto,
    TaskPatch, TaskStatus,
};
use focus_core::memory::MemoryStore;
use focus_core::{
    Clock as _, CoreError, CoreResult, DomainEvent, FocusService, ImportedItem, ImportedTask,
    ManualClock, TaskSources,
};
use futures_executor::block_on;

fn t0() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 10, 24, 9, 0, 0).unwrap()
}

struct Env {
    clock: Arc<ManualClock>,
    store: MemoryStore,
    svc: FocusService,
}

impl Env {
    fn new() -> Self {
        Self::at(ManualClock::new(t0()))
    }

    fn at(clock: ManualClock) -> Self {
        let clock = Arc::new(clock);
        let store = MemoryStore::new();
        let svc = FocusService::new(store.ports(clock.clone()));
        Self { clock, store, svc }
    }

    fn adv(&self, min: i64) {
        self.clock.advance(Duration::minutes(min));
    }

    fn item(&self, title: &str) -> TaskDto {
        block_on(self.svc.create_task(NewTask::new(title))).unwrap()
    }
}

struct Clock<'a>(&'a ManualClock);
impl TestClock for Clock<'_> {
    fn advance(&self, by: Duration) {
        self.0.advance(by);
    }
}

#[test]
fn contract_scenarios_pass() {
    let env = Env::new();
    let results = block_on(run_all_with_clock(&env.svc, &Clock(&env.clock)));
    assert_eq!(results.len(), 11);
    assert_all_passed(&results).unwrap_or_else(|e| panic!("{e}"));
}

#[test]
fn inherent_futures_are_send() {
    fn send<T: Send>(_: T) {}
    let env = Env::new();
    send(env.svc.status());
    send(env.svc.start_work(env.item("x").id));
    send(env.svc.tick(t0()));
    send(env.svc.import("u"));
}

#[test]
fn work_limit_closes_with_limit_and_waits_for_rest() {
    let env = Env::new();
    let item = env.item("пункт");
    block_on(env.svc.start_work(item.id)).unwrap();

    env.adv(49);
    assert!(block_on(env.svc.tick(env.clock.now())).unwrap().is_empty());

    env.adv(2); // 9:51
    let events = block_on(env.svc.tick(env.clock.now())).unwrap();
    let ServerEvent::SessionEnded { session } = &events[0] else {
        panic!("{events:?}")
    };
    assert_eq!(session.kind, SessionKind::Work);
    assert_eq!(session.end_reason, Some(EndReason::Limit));
    assert_eq!(session.ended_at, Some(t0() + Duration::minutes(50)));
    assert_eq!(session.duration_sec, 50 * 60);
    assert!(matches!(events.last(), Some(ServerEvent::StatusChanged(_))));

    // Отдых сам не стартует: ждём «Начать отдых», время теряется.
    let status = block_on(env.svc.status()).unwrap();
    assert_eq!(status.active, None);
    assert!(status.rest_due);
    assert_eq!(status.idle_since, Some(t0() + Duration::minutes(50)));
    // Пункт остаётся открытым (в работе), не done.
    let tree = block_on(env.svc.list_tasks()).unwrap();
    assert_eq!(tree[0].status, TaskStatus::InProgress);
    assert_eq!(status.worked_today_sec, 50 * 60);

    env.adv(2); // 9:53 — пользователь нажал «Начать отдых»
    let status = block_on(env.svc.start_rest()).unwrap();
    assert!(!status.rest_due);
    match status.active {
        Some(ActiveSession::Rest {
            started_at,
            ends_at,
            ..
        }) => {
            assert_eq!(started_at, t0() + Duration::minutes(53));
            assert_eq!(ends_at, t0() + Duration::minutes(63));
        }
        other => panic!("ожидался отдых: {other:?}"),
    }
    let day = block_on(env.svc.day(status.day.unwrap().date)).unwrap();
    assert_eq!(day.totals.lost_sec, 3 * 60);
}

#[test]
fn rest_ends_by_itself_then_time_is_lost() {
    let env = Env::new();
    let item = env.item("пункт");
    block_on(env.svc.start_work(item.id)).unwrap();
    // Таймер не вызывался 75 минут: работа закрыта ровно на 50-й минуте.
    env.adv(75);
    let events = block_on(env.svc.tick(env.clock.now())).unwrap();
    let ended: Vec<_> = events
        .iter()
        .filter_map(|e| match e {
            ServerEvent::SessionEnded { session } => Some((session.kind, session.end_reason)),
            _ => None,
        })
        .collect();
    assert_eq!(ended, [(SessionKind::Work, Some(EndReason::Limit))]);
    block_on(env.svc.start_rest()).unwrap(); // 10:15
    env.adv(15);
    let events = block_on(env.svc.tick(env.clock.now())).unwrap();
    assert!(events.iter().any(|e| matches!(
        e,
        ServerEvent::SessionEnded { session }
            if session.kind == SessionKind::Rest && session.end_reason == Some(EndReason::Limit)
    )));
    let status = block_on(env.svc.status()).unwrap();
    assert_eq!(status.active, None);
    assert!(!status.rest_due);
    assert_eq!(status.idle_since, Some(t0() + Duration::minutes(85)));

    let day = block_on(env.svc.day(status.day.unwrap().date)).unwrap();
    assert_eq!(day.totals.work_sec, 50 * 60);
    assert_eq!(day.totals.rest_sec, 10 * 60);
    // Ожидание «Начать отдых» (25 мин) и хвост после отдыха (5 мин).
    assert_eq!(day.totals.lost_sec, 30 * 60);
    assert_eq!(day.lost.len(), 2);
    assert_eq!(day.lost[0].start, t0() + Duration::minutes(50));
    assert_eq!(day.lost[1].start, t0() + Duration::minutes(85));
}

#[test]
fn status_catches_up_without_tick_and_tick_reports_later() {
    let env = Env::new();
    let item = env.item("пункт");
    block_on(env.svc.start_work(item.id)).unwrap();
    env.adv(55);
    // Без tick статус уже честный: работа закрыта, отдых ждёт.
    let status = block_on(env.svc.status()).unwrap();
    assert_eq!(status.active, None);
    assert!(status.rest_due);
    // А событие о конце работы отдаёт следующий tick.
    let events = block_on(env.svc.tick(env.clock.now())).unwrap();
    assert!(events.iter().any(|e| matches!(
        e,
        ServerEvent::SessionEnded { session } if session.end_reason == Some(EndReason::Limit)
    )));
    assert!(block_on(env.svc.tick(env.clock.now())).unwrap().is_empty());
}

#[test]
fn rest_cannot_be_skipped() {
    let env = Env::new();
    block_on(env.svc.start_rest()).unwrap();
    assert_eq!(
        block_on(env.svc.stop()).unwrap_err(),
        CoreError::WrongSessionKind {
            expected: SessionKind::Planning,
            actual: SessionKind::Rest
        }
    );
    assert_eq!(
        block_on(env.svc.start_planning()).unwrap_err(),
        CoreError::SessionAlreadyActive
    );
    assert_eq!(
        block_on(env.svc.done(DoneNext::Rest)).unwrap_err(),
        CoreError::WrongSessionKind {
            expected: SessionKind::Work,
            actual: SessionKind::Rest
        }
    );
    assert_eq!(
        block_on(env.svc.close_day()).unwrap_err(),
        CoreError::SessionAlreadyActive
    );
}

#[test]
fn rest_length_is_fixed_regardless_of_work() {
    let env = Env::new();
    let item = env.item("пункт");
    block_on(env.svc.start_work(item.id)).unwrap();
    env.adv(3);
    let res = block_on(env.svc.done(DoneNext::Rest)).unwrap();
    assert_eq!(res.ended.duration_sec, 3 * 60);
    let Some(ActiveSession::Rest {
        started_at,
        ends_at,
        ..
    }) = res.status.active
    else {
        panic!()
    };
    assert_eq!((ends_at - started_at).num_minutes(), 10);
}

#[test]
fn planning_switches_to_work_without_gap_and_counts() {
    let env = Env::new();
    let item = env.item("пункт");
    block_on(env.svc.start_planning()).unwrap();
    env.adv(7);
    let status = block_on(env.svc.start_work(item.id)).unwrap();
    let Some(ActiveSession::Work { started_at, .. }) = status.active else {
        panic!()
    };
    assert_eq!(started_at, t0() + Duration::minutes(7));
    let day = block_on(env.svc.day(status.day.unwrap().date)).unwrap();
    assert_eq!(day.sessions[0].end_reason, Some(EndReason::Switched));
    assert_eq!(day.totals.planning_sec, 7 * 60);
    assert_eq!(day.totals.lost_sec, 0);
    assert_eq!(status.worked_today_sec, 7 * 60);
}

#[test]
fn planning_has_no_limit() {
    let env = Env::new();
    block_on(env.svc.start_planning()).unwrap();
    env.adv(300);
    assert!(block_on(env.svc.tick(env.clock.now())).unwrap().is_empty());
    let status = block_on(env.svc.status()).unwrap();
    assert!(matches!(
        status.active,
        Some(ActiveSession::Planning { .. })
    ));
    assert_eq!(status.worked_today_sec, 300 * 60);
    assert_eq!(block_on(env.svc.next_deadline()).unwrap(), None);
}

#[test]
fn lost_time_before_first_session_after_manual_open() {
    let env = Env::new();
    block_on(env.svc.open_day()).unwrap();
    env.adv(20);
    let st = block_on(env.svc.status()).unwrap();
    assert_eq!(st.idle_since, Some(t0()));
    block_on(env.svc.start_planning()).unwrap();
    env.adv(10);
    let day = block_on(env.svc.day(st.day.unwrap().date)).unwrap();
    assert_eq!(day.totals.lost_sec, 20 * 60);
}

#[test]
fn session_across_midnight_belongs_to_start_day() {
    // Пояс +07:00: 23:30 местного = 16:30 UTC.
    let tz = FixedOffset::east_opt(7 * 3600).unwrap();
    let start = Utc.with_ymd_and_hms(2026, 10, 24, 16, 30, 0).unwrap();
    let env = Env::at(ManualClock::with_offset(start, tz));
    let item = env.item("ночной");
    let st = block_on(env.svc.start_planning()).unwrap();
    let day1 = st.day.unwrap();
    assert_eq!(day1.date, NaiveDate::from_ymd_opt(2026, 10, 24).unwrap());

    env.adv(45); // 00:15 местного
    let st = block_on(env.svc.status()).unwrap();
    assert_eq!(
        st.day.as_ref().unwrap().id,
        day1.id,
        "активная сессия держит вчерашний день"
    );
    assert_eq!(st.worked_today_sec, 45 * 60);
    let res = block_on(env.svc.stop()).unwrap();
    assert_eq!(res.ended.day_id, day1.id);

    // Сессия кончилась — «сегодня» уже новая дата, день ещё не открыт.
    let st = block_on(env.svc.status()).unwrap();
    assert_eq!(st.day, None);
    assert_eq!(st.worked_today_sec, 0);
    let st = block_on(env.svc.start_work(item.id)).unwrap();
    let day2 = st.day.unwrap();
    assert_eq!(day2.date, NaiveDate::from_ymd_opt(2026, 10, 25).unwrap());
    assert_ne!(day2.id, day1.id);

    let old = block_on(env.svc.day(day1.date)).unwrap();
    assert_eq!(old.totals.planning_sec, 45 * 60);
    assert_eq!(old.sessions.len(), 1);
}

#[test]
fn work_across_midnight_limit_keeps_rest_due() {
    let tz = FixedOffset::east_opt(7 * 3600).unwrap();
    let start = Utc.with_ymd_and_hms(2026, 10, 24, 16, 40, 0).unwrap(); // 23:40
    let env = Env::at(ManualClock::with_offset(start, tz));
    let item = env.item("x");
    let day1 = block_on(env.svc.start_work(item.id)).unwrap().day.unwrap();
    env.adv(55); // 00:35 — работа кончилась в 00:30, уже в новых сутках
    let st = block_on(env.svc.status()).unwrap();
    assert_eq!(st.active, None);
    assert!(st.rest_due, "{st:?}");
    let old = block_on(env.svc.day(day1.date)).unwrap();
    assert_eq!(old.sessions.len(), 1);
    assert_eq!(old.totals.work_sec, 50 * 60);
    // Отдых — уже в сегодняшнем дне, и ожидание снято.
    let st = block_on(env.svc.start_rest()).unwrap();
    assert!(!st.rest_due);
    assert_ne!(st.day.unwrap().id, day1.id);
}

#[test]
fn done_marks_item_and_publishes_events() {
    let env = Env::new();
    let parent = env.item("задача");
    let item = block_on(env.svc.create_task(NewTask::child(parent.id, "пункт"))).unwrap();
    block_on(env.svc.start_work(item.id)).unwrap();
    env.adv(10);
    block_on(env.svc.done(DoneNext::Planning)).unwrap();
    let events = env.store.outbox.take();
    let names: Vec<_> = events.iter().map(DomainEvent::name).collect();
    assert_eq!(
        names,
        [
            "item_created",
            "item_created",
            "session_closed",
            "item_completed"
        ]
    );
    match &events[2] {
        DomainEvent::SessionClosed {
            session,
            item: Some(i),
            parent: Some(p),
        } => {
            assert_eq!(session.end_reason, Some(EndReason::TaskDone));
            assert_eq!(i.id, item.id);
            assert_eq!(p.id, parent.id);
        }
        other => panic!("{other:?}"),
    }
    // Закрытый пункт снова в работу не берётся.
    block_on(env.svc.stop()).unwrap();
    assert!(matches!(
        block_on(env.svc.start_work(item.id)),
        Err(CoreError::Validation(_))
    ));
}

#[test]
fn interrupt_keeps_item_open_and_closes_session() {
    let env = Env::new();
    let item = env.item("пункт");
    block_on(env.svc.start_work(item.id)).unwrap();
    env.adv(5);
    let res = block_on(env.svc.interrupt()).unwrap();
    assert_eq!(res.ended.end_reason, Some(EndReason::Interrupted));
    assert_eq!(res.ended.duration_sec, 300);
    assert_eq!(res.status.idle_since, Some(t0() + Duration::minutes(5)));
    let tree = block_on(env.svc.list_tasks()).unwrap();
    assert_eq!(tree[0].status, TaskStatus::InProgress);
    // Команда ответила сама — tick не дублирует закрытие.
    assert!(block_on(env.svc.tick(env.clock.now())).unwrap().is_empty());
}

#[test]
fn tasks_tree_crud() {
    let env = Env::new();
    let parent = env.item("задача");
    let a = block_on(env.svc.create_task(NewTask::child(parent.id, "a"))).unwrap();
    let b = block_on(env.svc.create_task(NewTask::child(parent.id, "b"))).unwrap();
    assert_eq!((a.sort_order, b.sort_order), (0, 1));
    // Третий уровень запрещён.
    assert!(matches!(
        block_on(env.svc.create_task(NewTask::child(a.id, "c"))),
        Err(CoreError::Validation(_))
    ));
    assert!(matches!(
        block_on(env.svc.create_task(NewTask::new("   "))),
        Err(CoreError::Validation(_))
    ));
    let date = NaiveDate::from_ymd_opt(2026, 10, 25).unwrap();
    let patched = block_on(env.svc.update_task(
        b.id,
        TaskPatch {
            title: Some("b2".into()),
            notes: Some(Some("заметка".into())),
            planned_for: Some(Some(date)),
            sort_order: Some(-1),
            ..TaskPatch::default()
        },
    ))
    .unwrap();
    assert_eq!(patched.title, "b2");
    assert_eq!(patched.planned_for, Some(date));
    let cleared = block_on(env.svc.update_task(
        b.id,
        TaskPatch {
            notes: Some(None),
            status: Some(TaskStatus::Done),
            ..TaskPatch::default()
        },
    ))
    .unwrap();
    assert_eq!(cleared.notes, None);
    assert!(cleared.done_at.is_some());
    let reopened = block_on(env.svc.update_task(
        b.id,
        TaskPatch {
            status: Some(TaskStatus::Planned),
            ..TaskPatch::default()
        },
    ))
    .unwrap();
    assert_eq!(reopened.done_at, None);

    let tree = block_on(env.svc.list_tasks()).unwrap();
    let kids: Vec<_> = tree[0].children.iter().map(|c| c.title.as_str()).collect();
    assert_eq!(kids, ["b2", "a"]);
    let upd = block_on(env.svc.update_task(parent.id, TaskPatch::default())).unwrap();
    assert_eq!(upd.children.len(), 2);
    assert!(matches!(
        block_on(env.svc.update_task(
            focus_api_types::TaskId::from(focus_api_types::Uuid::nil()),
            TaskPatch::default()
        )),
        Err(CoreError::ItemNotFound)
    ));
    let completed = env
        .store
        .outbox
        .events()
        .iter()
        .filter(|e| e.name() == "item_completed")
        .count();
    assert_eq!(completed, 1);
}

#[test]
fn settings_apply_to_new_sessions_and_days_only() {
    let env = Env::new();
    let item = env.item("x");
    block_on(env.svc.start_work(item.id)).unwrap();
    let new = SettingsDto {
        work_min: 25,
        rest_min: 5,
        daily_target_min: 360,
        ..SettingsDto::default()
    };
    assert_eq!(block_on(env.svc.update_settings(new.clone())).unwrap(), new);
    let st = block_on(env.svc.status()).unwrap();
    let Some(ActiveSession::Work {
        started_at,
        ends_at,
        ..
    }) = st.active
    else {
        panic!()
    };
    assert_eq!(
        (ends_at - started_at).num_minutes(),
        50,
        "лимит зафиксирован при старте"
    );
    assert_eq!(
        st.target_sec,
        8 * 3600,
        "норма дня зафиксирована при открытии"
    );
    block_on(env.svc.interrupt()).unwrap();
    block_on(env.svc.start_work(item.id)).unwrap();
    let st = block_on(env.svc.status()).unwrap();
    let ends = st.active.unwrap().ends_at().unwrap();
    assert_eq!((ends - env.clock.now()).num_minutes(), 25);

    let bad = SettingsDto {
        work_min: 0,
        ..SettingsDto::default()
    };
    assert!(matches!(
        block_on(env.svc.update_settings(bad)),
        Err(CoreError::Validation(_))
    ));
}

#[test]
fn closed_day_reopens_and_close_requires_open_day() {
    let env = Env::new();
    assert!(matches!(
        block_on(env.svc.close_day()),
        Err(CoreError::Validation(_))
    ));
    let d = block_on(env.svc.open_day()).unwrap().day.unwrap();
    env.adv(30);
    let closed = block_on(env.svc.close_day()).unwrap().day.unwrap();
    assert_eq!(closed.closed_at, Some(t0() + Duration::minutes(30)));
    assert_eq!(
        block_on(env.svc.start_rest()).unwrap_err(),
        CoreError::DayClosed
    );
    // У закрытого дня хвост потерянного — до закрытия.
    let day = block_on(env.svc.day(d.date)).unwrap();
    assert_eq!(day.totals.lost_sec, 30 * 60);
    let again = block_on(env.svc.open_day()).unwrap();
    assert_eq!(again.day.unwrap().id, d.id);
    assert!(again.idle_since.is_some());
}

#[test]
fn unknown_date_gives_empty_day() {
    let env = Env::new();
    let date = NaiveDate::from_ymd_opt(2020, 1, 1).unwrap();
    let day = block_on(env.svc.day(date)).unwrap();
    assert_eq!(day.day, None);
    assert!(day.sessions.is_empty());
    assert_eq!(day.totals.target_sec, 8 * 3600);
}

struct FakeSource;

#[async_trait::async_trait]
impl TaskSources for FakeSource {
    async fn import(&self, url: &str) -> CoreResult<ImportedTask> {
        let mut items = vec![
            ImportedItem {
                title: "раз".into(),
                status: TaskStatus::Done,
            },
            ImportedItem {
                title: "два".into(),
                status: TaskStatus::Planned,
            },
        ];
        if url.ends_with("v2") {
            items.push(ImportedItem {
                title: "три".into(),
                status: TaskStatus::Cancelled,
            });
        }
        Ok(ImportedTask {
            source: "fake".into(),
            url: "https://jira/PROJ-1".into(),
            title: "PROJ-1 задача".into(),
            notes: None,
            items,
        })
    }
}

#[test]
fn import_creates_then_adds_new_items_only() {
    let clock = Arc::new(ManualClock::new(t0()));
    let store = MemoryStore::new();
    let svc = FocusService::new(store.ports_with_sources(clock, Arc::new(FakeSource)));
    let first = block_on(svc.import("PROJ-1")).unwrap();
    assert!(first.created);
    assert_eq!(first.added_items, 2);
    assert_eq!(first.task.children[0].status, TaskStatus::Done);
    assert_eq!(
        first.task.external_url.as_deref(),
        Some("https://jira/PROJ-1")
    );
    let second = block_on(svc.import("PROJ-1 v2")).unwrap();
    assert!(!second.created);
    assert_eq!(second.added_items, 1);
    assert_eq!(second.task.id, first.task.id);
    assert_eq!(second.task.children.len(), 3);
    assert_eq!(block_on(svc.list_tasks()).unwrap().len(), 1);
    // Импорт не публикует ItemCreated (пункты уже есть в источнике).
    let names: Vec<_> = store
        .outbox
        .events()
        .iter()
        .map(DomainEvent::name)
        .collect();
    assert_eq!(names, ["task_imported", "task_imported"]);
}

#[test]
fn import_without_sources_is_plugin_unavailable() {
    let env = Env::new();
    assert!(matches!(
        block_on(env.svc.import("PROJ-1")),
        Err(CoreError::PluginUnavailable(_))
    ));
    assert!(block_on(env.svc.plugin_status()).unwrap().is_empty());
}

#[test]
fn domain_event_json_round_trip() {
    let env = Env::new();
    env.item("x");
    let ev = env.store.outbox.take().remove(0);
    let json = serde_json::to_value(&ev).unwrap();
    assert_eq!(json["type"], "item_created");
    let back: DomainEvent = serde_json::from_value(json).unwrap();
    assert_eq!(back, ev);
    let unknown: DomainEvent =
        serde_json::from_value(serde_json::json!({"type": "future_event"})).unwrap();
    assert_eq!(unknown, DomainEvent::Unknown);
}
