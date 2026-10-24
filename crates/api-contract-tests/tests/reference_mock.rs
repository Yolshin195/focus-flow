//! Минимальная эталонная реализация `FocusApi` в памяти — только чтобы
//! проверить, что сами сценарии непротиворечивы. Настоящие реализации —
//! `MockApi` в `apps/ui` и сервис (шаг 4).

use std::cell::{Cell, RefCell};
use std::future::Future;
use std::pin::pin;
use std::task::{Context, Poll, Waker};

use async_trait::async_trait;
use chrono::{DateTime, Duration, NaiveDate, TimeZone, Utc};
use focus_api_contract_tests::{TestClock, assert_all_passed, run_all, run_all_with_clock};
use focus_api_types::{
    ActiveSession, ApiError, ApiResult, DayDto, DayId, DayInfoDto, DayTotalsDto, DoneNext,
    EndReason, FocusApi, ImportResult, ItemRef, NewTask, PluginStatusDto, SessionDto,
    SessionEndResult, SessionId, SessionKind, SettingsDto, StatusDto, TaskDto, TaskId, TaskPatch,
    TaskStatus, Uuid,
};

#[derive(Default)]
struct State {
    settings: SettingsDto,
    day: Option<DayInfoDto>,
    tasks: Vec<TaskDto>,
    sessions: Vec<SessionDto>,
    next_id: u128,
}

struct Mock {
    now: Cell<DateTime<Utc>>,
    s: RefCell<State>,
}

impl Mock {
    fn new() -> Self {
        Self {
            now: Cell::new(Utc.with_ymd_and_hms(2026, 10, 24, 9, 0, 0).unwrap()),
            s: RefCell::default(),
        }
    }

    fn id(s: &mut State) -> Uuid {
        s.next_id += 1;
        Uuid::from_u128(s.next_id)
    }

    fn duration(&self, x: &SessionDto) -> i64 {
        (x.ended_at.unwrap_or(self.now.get()) - x.started_at).num_seconds()
    }

    fn status(&self) -> StatusDto {
        let s = self.s.borrow();
        let now = self.now.get();
        let active = s.sessions.iter().find(|x| x.ended_at.is_none()).map(|x| {
            let ends_at = x.started_at + Duration::seconds(x.limit_sec.unwrap_or(0));
            match x.kind {
                SessionKind::Work => {
                    let task = s.tasks.iter().find(|t| Some(t.id) == x.task_id).unwrap();
                    ActiveSession::Work {
                        session_id: x.id,
                        item: ItemRef {
                            id: task.id,
                            title: task.title.clone(),
                            parent_id: task.parent_id,
                            parent_title: None,
                        },
                        started_at: x.started_at,
                        ends_at,
                    }
                }
                SessionKind::Rest => ActiveSession::Rest {
                    session_id: x.id,
                    started_at: x.started_at,
                    ends_at,
                },
                SessionKind::Planning => ActiveSession::Planning {
                    session_id: x.id,
                    started_at: x.started_at,
                },
            }
        });
        StatusDto {
            server_time: now,
            day: s.day.clone(),
            active,
            worked_today_sec: s
                .sessions
                .iter()
                .filter(|x| x.kind != SessionKind::Rest)
                .map(|x| self.duration(x))
                .sum(),
            target_sec: i64::from(s.settings.daily_target_min) * 60,
            idle_since: None,
        }
    }

    fn start(&self, kind: SessionKind, task_id: Option<TaskId>) -> ApiResult<StatusDto> {
        {
            let mut s = self.s.borrow_mut();
            if let Some(a) = s.sessions.iter().find(|x| x.ended_at.is_none()) {
                // Планирование закрывается стартом работы или отдыха.
                if a.kind != SessionKind::Planning || kind == SessionKind::Planning {
                    return Err(ApiError::SessionAlreadyActive);
                }
            }
            if let Some(id) = task_id {
                let task = s
                    .tasks
                    .iter()
                    .find(|t| t.id == id)
                    .ok_or(ApiError::ItemNotFound)?;
                if s.tasks.iter().any(|t| t.parent_id == Some(task.id)) {
                    return Err(ApiError::ItemRequired);
                }
            }
            let now = self.now.get();
            match &s.day {
                Some(d) if d.closed_at.is_some() => return Err(ApiError::DayClosed),
                Some(_) => {}
                None => {
                    let id = DayId::from(Self::id(&mut s));
                    let target_sec = i64::from(s.settings.daily_target_min) * 60;
                    s.day = Some(DayInfoDto {
                        id,
                        date: now.date_naive(),
                        opened_at: now,
                        closed_at: None,
                        target_sec,
                    });
                }
            }
            if let Some(a) = s.sessions.iter_mut().find(|x| x.ended_at.is_none()) {
                a.ended_at = Some(now);
                a.end_reason = Some(EndReason::Switched);
            }
            let limit_sec = match kind {
                SessionKind::Work => Some(i64::from(s.settings.work_min) * 60),
                SessionKind::Rest => Some(i64::from(s.settings.rest_min) * 60),
                SessionKind::Planning => None,
            };
            let id = SessionId::from(Self::id(&mut s));
            let day_id = s.day.as_ref().unwrap().id;
            s.sessions.push(SessionDto {
                id,
                day_id,
                kind,
                task_id,
                task_title: None,
                started_at: now,
                ended_at: None,
                limit_sec,
                end_reason: None,
                duration_sec: 0,
            });
        }
        Ok(self.status())
    }

    fn end(&self, expected: SessionKind, reason: EndReason) -> ApiResult<SessionDto> {
        let mut s = self.s.borrow_mut();
        let now = self.now.get();
        let a = s
            .sessions
            .iter_mut()
            .find(|x| x.ended_at.is_none())
            .ok_or(ApiError::NoActiveSession)?;
        if a.kind != expected {
            return Err(ApiError::WrongSessionKind {
                expected,
                actual: a.kind,
            });
        }
        a.ended_at = Some(now);
        a.end_reason = Some(reason);
        a.duration_sec = (now - a.started_at).num_seconds();
        Ok(a.clone())
    }

    fn tree(&self) -> Vec<TaskDto> {
        let s = self.s.borrow();
        let build = |p: &TaskDto| TaskDto {
            children: s
                .tasks
                .iter()
                .filter(|c| c.parent_id == Some(p.id))
                .cloned()
                .collect(),
            ..p.clone()
        };
        s.tasks
            .iter()
            .filter(|t| t.parent_id.is_none())
            .map(build)
            .collect()
    }
}

impl TestClock for Mock {
    fn advance(&self, by: Duration) {
        self.now.set(self.now.get() + by);
    }
}

#[async_trait(?Send)]
impl FocusApi for Mock {
    async fn get_status(&self) -> ApiResult<StatusDto> {
        Ok(self.status())
    }
    async fn open_day(&self) -> ApiResult<StatusDto> {
        {
            let mut s = self.s.borrow_mut();
            let now = self.now.get();
            match &mut s.day {
                Some(d) => d.closed_at = None,
                None => {
                    let id = DayId::from(Self::id(&mut s));
                    s.day = Some(DayInfoDto {
                        id,
                        date: now.date_naive(),
                        opened_at: now,
                        closed_at: None,
                        target_sec: 8 * 3600,
                    });
                }
            }
        }
        Ok(self.status())
    }
    async fn close_day(&self) -> ApiResult<StatusDto> {
        if self.status().active.is_some() {
            return Err(ApiError::SessionAlreadyActive);
        }
        if let Some(d) = &mut self.s.borrow_mut().day {
            d.closed_at = Some(self.now.get());
        }
        Ok(self.status())
    }
    async fn get_day(&self, date: NaiveDate) -> ApiResult<DayDto> {
        let s = self.s.borrow();
        let sessions: Vec<SessionDto> = s
            .sessions
            .iter()
            .map(|x| SessionDto {
                duration_sec: self.duration(x),
                ..x.clone()
            })
            .collect();
        let sum = |k| {
            sessions
                .iter()
                .filter(|x| x.kind == k)
                .map(|x| x.duration_sec)
                .sum()
        };
        let (work_sec, planning_sec, rest_sec) = (
            sum(SessionKind::Work),
            sum(SessionKind::Planning),
            sum(SessionKind::Rest),
        );
        Ok(DayDto {
            date,
            day: s.day.clone(),
            sessions,
            lost: vec![],
            totals: DayTotalsDto {
                work_sec,
                planning_sec,
                rest_sec,
                lost_sec: 0,
                worked_sec: work_sec + planning_sec,
                target_sec: 8 * 3600,
            },
        })
    }
    async fn list_tasks(&self) -> ApiResult<Vec<TaskDto>> {
        Ok(self.tree())
    }
    async fn create_task(&self, task: NewTask) -> ApiResult<TaskDto> {
        let mut s = self.s.borrow_mut();
        let dto = TaskDto {
            id: TaskId::from(Self::id(&mut s)),
            parent_id: task.parent_id,
            title: task.title,
            notes: task.notes,
            status: TaskStatus::Planned,
            planned_for: task.planned_for,
            estimate_intervals: task.estimate_intervals,
            sort_order: 0,
            created_at: self.now.get(),
            done_at: None,
            external_url: None,
            children: vec![],
        };
        s.tasks.push(dto.clone());
        Ok(dto)
    }
    async fn update_task(&self, _id: TaskId, _patch: TaskPatch) -> ApiResult<TaskDto> {
        Err(ApiError::Internal("не нужно сценариям".into()))
    }
    async fn start_work(&self, item_id: TaskId) -> ApiResult<StatusDto> {
        self.start(SessionKind::Work, Some(item_id))
    }
    async fn start_planning(&self) -> ApiResult<StatusDto> {
        self.start(SessionKind::Planning, None)
    }
    async fn start_rest(&self) -> ApiResult<StatusDto> {
        self.start(SessionKind::Rest, None)
    }
    async fn done(&self, next: DoneNext) -> ApiResult<SessionEndResult> {
        let ended = self.end(SessionKind::Work, EndReason::TaskDone)?;
        {
            let mut s = self.s.borrow_mut();
            let task = s
                .tasks
                .iter_mut()
                .find(|t| Some(t.id) == ended.task_id)
                .unwrap();
            task.status = TaskStatus::Done;
            task.done_at = Some(self.now.get());
        }
        let status = match next {
            DoneNext::Rest => self.start(SessionKind::Rest, None)?,
            DoneNext::Planning => self.start(SessionKind::Planning, None)?,
        };
        Ok(SessionEndResult { ended, status })
    }
    async fn interrupt(&self) -> ApiResult<SessionEndResult> {
        let ended = self.end(SessionKind::Work, EndReason::Interrupted)?;
        Ok(SessionEndResult {
            ended,
            status: self.status(),
        })
    }
    async fn stop(&self) -> ApiResult<SessionEndResult> {
        let ended = self.end(SessionKind::Planning, EndReason::Stopped)?;
        Ok(SessionEndResult {
            ended,
            status: self.status(),
        })
    }
    async fn import(&self, _url: String) -> ApiResult<ImportResult> {
        Err(ApiError::PluginUnavailable("нет плагинов".into()))
    }
    async fn get_settings(&self) -> ApiResult<SettingsDto> {
        Ok(self.s.borrow().settings.clone())
    }
    async fn update_settings(&self, settings: SettingsDto) -> ApiResult<SettingsDto> {
        self.s.borrow_mut().settings = settings.clone();
        Ok(settings)
    }
    async fn plugin_status(&self) -> ApiResult<Vec<PluginStatusDto>> {
        Ok(vec![])
    }
}

/// Все future эталонного мока готовы сразу — исполнитель не нужен.
fn block_on<F: Future>(fut: F) -> F::Output {
    let mut fut = pin!(fut);
    let mut cx = Context::from_waker(Waker::noop());
    loop {
        if let Poll::Ready(v) = fut.as_mut().poll(&mut cx) {
            return v;
        }
    }
}

#[test]
fn scenarios_pass_on_reference_mock() {
    let mock = Mock::new();
    let results = block_on(run_all(&mock));
    assert_eq!(results.len(), 10);
    assert_all_passed(&results).unwrap_or_else(|e| panic!("{e}"));
}

#[test]
fn scenarios_pass_on_reference_mock_with_clock() {
    let mock = Mock::new();
    let results = block_on(run_all_with_clock(&mock, &mock));
    assert_all_passed(&results).unwrap_or_else(|e| panic!("{e}"));
}

#[test]
fn broken_backend_is_reported() {
    struct Broken(Mock);
    #[async_trait(?Send)]
    impl FocusApi for Broken {
        async fn get_status(&self) -> ApiResult<StatusDto> {
            self.0.get_status().await
        }
        async fn open_day(&self) -> ApiResult<StatusDto> {
            self.0.open_day().await
        }
        async fn close_day(&self) -> ApiResult<StatusDto> {
            self.0.close_day().await
        }
        async fn get_day(&self, d: NaiveDate) -> ApiResult<DayDto> {
            self.0.get_day(d).await
        }
        async fn list_tasks(&self) -> ApiResult<Vec<TaskDto>> {
            self.0.list_tasks().await
        }
        async fn create_task(&self, t: NewTask) -> ApiResult<TaskDto> {
            self.0.create_task(t).await
        }
        async fn update_task(&self, i: TaskId, p: TaskPatch) -> ApiResult<TaskDto> {
            self.0.update_task(i, p).await
        }
        // Ошибка: работа без проверки пункта.
        async fn start_work(&self, _i: TaskId) -> ApiResult<StatusDto> {
            self.0.start_planning().await
        }
        async fn start_planning(&self) -> ApiResult<StatusDto> {
            self.0.start_planning().await
        }
        async fn start_rest(&self) -> ApiResult<StatusDto> {
            self.0.start_rest().await
        }
        async fn done(&self, n: DoneNext) -> ApiResult<SessionEndResult> {
            self.0.done(n).await
        }
        async fn interrupt(&self) -> ApiResult<SessionEndResult> {
            self.0.interrupt().await
        }
        async fn stop(&self) -> ApiResult<SessionEndResult> {
            self.0.stop().await
        }
        async fn import(&self, u: String) -> ApiResult<ImportResult> {
            self.0.import(u).await
        }
        async fn get_settings(&self) -> ApiResult<SettingsDto> {
            self.0.get_settings().await
        }
        async fn update_settings(&self, s: SettingsDto) -> ApiResult<SettingsDto> {
            self.0.update_settings(s).await
        }
        async fn plugin_status(&self) -> ApiResult<Vec<PluginStatusDto>> {
            self.0.plugin_status().await
        }
    }
    let results = block_on(run_all(&Broken(Mock::new())));
    let err = assert_all_passed(&results).unwrap_err();
    assert!(err.contains("start_work_unknown_item_not_found"), "{err}");
}
