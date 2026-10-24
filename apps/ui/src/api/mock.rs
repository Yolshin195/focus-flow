//! `MockApi` — реализация [`FocusApi`] в памяти, без сервиса.
//!
//! Нужна, чтобы UI жил без `focus-flow serve`: живой таймер, демо-данные дня,
//! импорт «из Jira». Правила таймера — по SPEC (как в эталонном моке
//! `api-contract-tests`), сверяются сценариями контракта в тестах `ui`.
//!
//! Время мока — свои часы: `anchor + (сейчас - старт) * speed`. `speed = 0`
//! останавливает часы (их двигает только [`MockApi::advance`]), `speed > 1`
//! ускоряет демо. Истечение лимита обрабатывается лениво — при каждом вызове.

use std::cell::{Cell, RefCell};

use async_trait::async_trait;
use focus_api_types::chrono::{DateTime, Duration, Local, NaiveDate, NaiveTime, TimeZone, Utc};
use focus_api_types::{
    ActiveSession, ApiError, ApiResult, DayDto, DayId, DayInfoDto, DayTotalsDto, DoneNext,
    EndReason, FocusApi, ImportResult, ItemRef, NewTask, PluginStatusDto, SessionDto,
    SessionEndResult, SessionId, SessionKind, SettingsDto, StatusDto, TaskDto, TaskId, TaskPatch,
    TaskStatus, TimeRange, Uuid,
};

/// Строка демо-истории: вид, начало и конец (мин от начала дня), пункт, причина.
type Row = (SessionKind, i64, i64, Option<TaskId>, Option<EndReason>);
/// Активная сессия демо: вид, начало (мин), пункт.
type Active = (SessionKind, i64, Option<TaskId>);

/// Начальное состояние демо-данных.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MockScenario {
    /// Пусто: ни задач, ни дня (для тестов контракта).
    Empty,
    /// Прошедший день до 16:19, сейчас ничего не идёт — тикает потерянное время.
    #[default]
    Idle,
    /// Идёт работа над пунктом (12 минут из 50).
    Work,
    /// Идёт отдых после лимита работы.
    Rest,
    /// Идёт планирование.
    Planning,
}

impl MockScenario {
    /// Разбор значения переменной окружения (`work`, `rest`, `planning`, `idle`, `empty`).
    /// UI-состояния (`done-choice`, `interrupt`, `picker`) поверх работы/простоя
    /// тоже допустимы: им соответствует состояние API.
    pub fn parse(s: &str) -> Self {
        match s.trim() {
            "empty" => Self::Empty,
            "work" | "done-choice" | "interrupt" => Self::Work,
            "rest" => Self::Rest,
            "planning" => Self::Planning,
            _ => Self::Idle,
        }
    }
}

#[derive(Debug, Clone)]
struct Task {
    id: TaskId,
    parent_id: Option<TaskId>,
    title: String,
    notes: Option<String>,
    status: TaskStatus,
    planned_for: Option<NaiveDate>,
    estimate_intervals: Option<u32>,
    sort_order: i64,
    created_at: DateTime<Utc>,
    done_at: Option<DateTime<Utc>>,
    external_url: Option<String>,
}

#[derive(Debug, Clone)]
struct Session {
    id: SessionId,
    day_id: DayId,
    kind: SessionKind,
    task_id: Option<TaskId>,
    started_at: DateTime<Utc>,
    ended_at: Option<DateTime<Utc>>,
    limit_sec: Option<i64>,
    end_reason: Option<EndReason>,
}

impl Session {
    fn ends_at(&self) -> Option<DateTime<Utc>> {
        self.limit_sec
            .map(|l| self.started_at + Duration::seconds(l))
    }

    fn duration(&self, now: DateTime<Utc>) -> i64 {
        (self.ended_at.unwrap_or(now) - self.started_at)
            .num_seconds()
            .max(0)
    }
}

#[derive(Debug, Default)]
struct State {
    settings: SettingsDto,
    days: Vec<DayInfoDto>,
    tasks: Vec<Task>,
    sessions: Vec<Session>,
}

/// Часы мока.
struct Clock {
    anchor: Cell<DateTime<Utc>>,
    real_start: DateTime<Utc>,
    speed: f64,
}

impl Clock {
    fn now(&self) -> DateTime<Utc> {
        if self.speed == 0.0 {
            return self.anchor.get();
        }
        let real_ms = (Utc::now() - self.real_start).num_milliseconds() as f64;
        self.anchor.get() + Duration::milliseconds((real_ms * self.speed) as i64)
    }
}

/// `FocusApi` в памяти. Однопоточный (`RefCell`), как и весь UI.
pub struct MockApi {
    clock: Clock,
    s: RefCell<State>,
}

fn local_date(t: DateTime<Utc>) -> NaiveDate {
    t.with_timezone(&Local).date_naive()
}

fn new_id() -> Uuid {
    Uuid::now_v7()
}

/// Ключ задачи Jira (`ABC-123`) в ссылке или строке.
pub fn jira_key(s: &str) -> Option<String> {
    s.split(|c: char| !(c.is_ascii_alphanumeric() || c == '-'))
        .flat_map(|w| w.split('/'))
        .find_map(|w| {
            let (proj, num) = w.split_once('-')?;
            let ok = proj.len() >= 2
                && proj.starts_with(|c: char| c.is_ascii_uppercase())
                && proj
                    .chars()
                    .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
                && !num.is_empty()
                && num.chars().all(|c| c.is_ascii_digit());
            ok.then(|| w.to_string())
        })
}

impl MockApi {
    /// Пустой мок с остановленными часами (время двигает только [`Self::advance`]).
    pub fn frozen(now: DateTime<Utc>) -> Self {
        Self::build(now, 0.0)
    }

    /// Пустой мок на реальном времени.
    pub fn realtime() -> Self {
        Self::build(Utc::now(), 1.0)
    }

    /// Мок с демо-данными; `speed` — во сколько раз время мока быстрее реального.
    pub fn demo(scenario: MockScenario, speed: f64) -> Self {
        if scenario == MockScenario::Empty {
            return Self::build(Utc::now(), speed);
        }
        let start = Local::now()
            .date_naive()
            .and_time(NaiveTime::from_hms_opt(9, 12, 0).unwrap_or_default());
        let day_start = Local
            .from_local_datetime(&start)
            .earliest()
            .map(|t| t.with_timezone(&Utc))
            .unwrap_or_else(Utc::now);
        let mock = Self::build(day_start, speed);
        let now_min = mock.seed(day_start, scenario);
        mock.clock
            .anchor
            .set(day_start + Duration::minutes(now_min));
        mock
    }

    fn build(now: DateTime<Utc>, speed: f64) -> Self {
        Self {
            clock: Clock {
                anchor: Cell::new(now),
                real_start: Utc::now(),
                speed: speed.max(0.0),
            },
            s: RefCell::default(),
        }
    }

    /// Сдвинуть часы мока вперёд.
    pub fn advance(&self, by: Duration) {
        self.clock.anchor.set(self.clock.anchor.get() + by);
    }

    fn now(&self) -> DateTime<Utc> {
        self.clock.now()
    }

    /// Демо-история дня; возвращает «сейчас» в минутах от начала дня.
    fn seed(&self, t0: DateTime<Utc>, scenario: MockScenario) -> i64 {
        let mut s = self.s.borrow_mut();
        let at = |m: i64| t0 + Duration::minutes(m);
        let created = t0 - Duration::days(2);
        let add = |s: &mut State, parent: Option<TaskId>, title: &str, url: Option<&str>| {
            let id = TaskId::from(new_id());
            let order = s.tasks.iter().filter(|t| t.parent_id == parent).count() as i64;
            s.tasks.push(Task {
                id,
                parent_id: parent,
                title: title.into(),
                notes: None,
                status: TaskStatus::Planned,
                planned_for: None,
                estimate_intervals: None,
                sort_order: order,
                created_at: created,
                done_at: None,
                external_url: url.map(Into::into),
            });
            id
        };
        let jira = add(
            &mut s,
            None,
            "Агент тестирования",
            Some("https://jira.example.com/browse/PROJ-123"),
        );
        let items: Vec<TaskId> = [
            "Вердикт в PDF и в чате считается по-разному",
            "Название отчёта по тестированию — с временем",
            "Не выводить пароль приложения в историю чата",
            "Останавливать прогон при повторяющемся сбое",
            "Подключить LLM-роутер с вопросом о системе",
            "Проверить, что при повторном импорте задачи из Jira новые строки чек-листа догружаются без дублей, а изменённые строки сопоставляются по тексту и не теряют статус",
        ]
        .iter()
        .map(|t| add(&mut s, Some(jira), t, None))
        .collect();
        // Оценки, «на сегодня» и заметка — чтобы демо показывало их в списке.
        let today = local_date(t0);
        for (i, est, planned) in [(2, 1, true), (3, 2, true), (4, 1, false), (5, 2, false)] {
            if let Some(t) = s.tasks.iter_mut().find(|t| t.id == items[i]) {
                t.estimate_intervals = Some(est);
                t.planned_for = planned.then_some(today);
            }
        }
        if let Some(t) = s.tasks.iter_mut().find(|t| t.id == items[5]) {
            t.notes = Some(
                "После правки строки в Jira пункт задвоился. Проверить на копии чек-листа.".into(),
            );
        }
        let inbox = add(&mut s, None, "Разобрать входящие письма", None);
        add(&mut s, None, "Подготовить план на неделю", None);

        let day_id = DayId::from(new_id());
        s.days.push(DayInfoDto {
            id: day_id,
            date: local_date(t0),
            opened_at: t0,
            closed_at: None,
            target_sec: 8 * 3600,
        });

        use EndReason::{Interrupted, Limit, Stopped, Switched, TaskDone};
        use SessionKind::{Planning as P, Rest as R, Work as W};
        let history: [Row; 18] = [
            (P, 0, 18, None, Some(Switched)),
            (W, 18, 68, Some(items[0]), Some(Limit)),
            (R, 68, 78, None, Some(Limit)),
            (W, 86, 124, Some(items[0]), Some(TaskDone)),
            (R, 124, 134, None, Some(Limit)),
            (W, 139, 161, Some(items[1]), Some(Interrupted)),
            (P, 208, 220, None, Some(Switched)),
            (W, 220, 270, Some(items[1]), Some(Limit)),
            (R, 270, 280, None, Some(Limit)),
            (W, 284, 318, Some(items[1]), Some(TaskDone)),
            (R, 318, 328, None, Some(Limit)),
            (W, 333, 361, Some(inbox), Some(TaskDone)),
            (P, 361, 367, None, Some(Switched)),
            (W, 367, 417, Some(items[3]), Some(Limit)),
            (R, 417, 427, None, Some(Limit)),
            // Ниже — «хвост» под сценарий; закрытые записи без сценария не нужны.
            (W, 431, 481, Some(items[3]), Some(Limit)),
            (R, 481, 491, None, Some(Limit)),
            (P, 431, 443, None, Some(Stopped)),
        ];
        let (take, active, now): (usize, Option<Active>, i64) = match scenario {
            MockScenario::Work => (15, Some((W, 431, Some(items[3]))), 443),
            MockScenario::Rest => (16, Some((R, 481, None)), 484),
            MockScenario::Planning => (15, Some((P, 431, None)), 443),
            _ => (15, None, 431),
        };
        let limit = |k: SessionKind| match k {
            W => Some(50 * 60),
            R => Some(10 * 60),
            P => None,
        };
        for (kind, from, to, task_id, reason) in history.iter().take(take).copied() {
            s.sessions.push(Session {
                id: SessionId::from(new_id()),
                day_id,
                kind,
                task_id,
                started_at: at(from),
                ended_at: Some(at(to)),
                limit_sec: limit(kind),
                end_reason: reason,
            });
        }
        if let Some((kind, from, task_id)) = active {
            s.sessions.push(Session {
                id: SessionId::from(new_id()),
                day_id,
                kind,
                task_id,
                started_at: at(from),
                ended_at: None,
                limit_sec: limit(kind),
                end_reason: None,
            });
        }
        for id in [items[0], items[1], inbox] {
            if let Some(t) = s.tasks.iter_mut().find(|t| t.id == id) {
                t.status = TaskStatus::Done;
                t.done_at = Some(at(124));
            }
        }
        if let Some(t) = s.tasks.iter_mut().find(|t| t.id == items[3]) {
            t.status = TaskStatus::InProgress;
        }
        now
    }

    /// Ленивая обработка лимитов: работа по лимиту → отдых, отдых по лимиту → простой.
    fn tick(&self, now: DateTime<Utc>) {
        let mut s = self.s.borrow_mut();
        loop {
            let Some(a) = s.sessions.iter_mut().find(|x| x.ended_at.is_none()) else {
                return;
            };
            let Some(ends_at) = a.ends_at() else { return };
            if now < ends_at {
                return;
            }
            a.ended_at = Some(ends_at);
            a.end_reason = Some(EndReason::Limit);
            let (kind, day_id) = (a.kind, a.day_id);
            if kind == SessionKind::Work {
                let rest = i64::from(s.settings.rest_min) * 60;
                s.sessions.push(Session {
                    id: SessionId::from(new_id()),
                    day_id,
                    kind: SessionKind::Rest,
                    task_id: None,
                    started_at: ends_at,
                    ended_at: None,
                    limit_sec: Some(rest),
                    end_reason: None,
                });
            }
        }
    }

    fn current_day(s: &State, now: DateTime<Utc>) -> Option<&DayInfoDto> {
        s.days.last().filter(|d| d.date == local_date(now))
    }

    fn item_ref(s: &State, id: TaskId) -> Option<ItemRef> {
        let t = s.tasks.iter().find(|t| t.id == id)?;
        let parent = t.parent_id.and_then(|p| s.tasks.iter().find(|x| x.id == p));
        Some(ItemRef {
            id: t.id,
            title: t.title.clone(),
            parent_id: t.parent_id,
            parent_title: parent.map(|p| p.title.clone()),
        })
    }

    fn status_at(&self, now: DateTime<Utc>) -> StatusDto {
        let s = self.s.borrow();
        let day = Self::current_day(&s, now).cloned();
        let active_session = s.sessions.iter().find(|x| x.ended_at.is_none());
        let active = active_session.and_then(|x| {
            Some(match x.kind {
                SessionKind::Work => ActiveSession::Work {
                    session_id: x.id,
                    item: Self::item_ref(&s, x.task_id?)?,
                    started_at: x.started_at,
                    ends_at: x.ends_at()?,
                },
                SessionKind::Rest => ActiveSession::Rest {
                    session_id: x.id,
                    started_at: x.started_at,
                    ends_at: x.ends_at()?,
                },
                SessionKind::Planning => ActiveSession::Planning {
                    session_id: x.id,
                    started_at: x.started_at,
                },
            })
        });
        let day_sessions = |d: &DayInfoDto| {
            s.sessions
                .iter()
                .filter(|x| x.day_id == d.id)
                .cloned()
                .collect::<Vec<_>>()
        };
        let (worked, idle_since) = match &day {
            Some(d) => {
                let list = day_sessions(d);
                let worked = list
                    .iter()
                    .filter(|x| x.kind != SessionKind::Rest)
                    .map(|x| x.duration(now))
                    .sum();
                let idle = (active.is_none() && d.closed_at.is_none()).then(|| {
                    list.iter()
                        .filter_map(|x| x.ended_at)
                        .max()
                        .unwrap_or(d.opened_at)
                        .max(d.opened_at)
                });
                (worked, idle)
            }
            None => (0, None),
        };
        StatusDto {
            server_time: now,
            target_sec: day
                .as_ref()
                .map(|d| d.target_sec)
                .unwrap_or(i64::from(s.settings.daily_target_min) * 60),
            day,
            active,
            worked_today_sec: worked,
            idle_since,
        }
    }

    fn ensure_day(s: &mut State, now: DateTime<Utc>) -> ApiResult<DayId> {
        match Self::current_day(s, now) {
            Some(d) if d.closed_at.is_some() => Err(ApiError::DayClosed),
            Some(d) => Ok(d.id),
            None => {
                let id = DayId::from(new_id());
                let target_sec = i64::from(s.settings.daily_target_min) * 60;
                s.days.push(DayInfoDto {
                    id,
                    date: local_date(now),
                    opened_at: now,
                    closed_at: None,
                    target_sec,
                });
                Ok(id)
            }
        }
    }

    fn start(
        &self,
        now: DateTime<Utc>,
        kind: SessionKind,
        task_id: Option<TaskId>,
    ) -> ApiResult<StatusDto> {
        {
            let mut s = self.s.borrow_mut();
            if let Some(a) = s.sessions.iter().find(|x| x.ended_at.is_none()) {
                // Планирование закрывается стартом работы или отдыха.
                if a.kind != SessionKind::Planning || kind == SessionKind::Planning {
                    return Err(ApiError::SessionAlreadyActive);
                }
            }
            if let Some(id) = task_id {
                if !s.tasks.iter().any(|t| t.id == id) {
                    return Err(ApiError::ItemNotFound);
                }
                if s.tasks.iter().any(|t| t.parent_id == Some(id)) {
                    return Err(ApiError::ItemRequired);
                }
            }
            let day_id = Self::ensure_day(&mut s, now)?;
            if let Some(a) = s.sessions.iter_mut().find(|x| x.ended_at.is_none()) {
                a.ended_at = Some(now);
                a.end_reason = Some(EndReason::Switched);
            }
            let limit_sec = match kind {
                SessionKind::Work => Some(i64::from(s.settings.work_min) * 60),
                SessionKind::Rest => Some(i64::from(s.settings.rest_min) * 60),
                SessionKind::Planning => None,
            };
            if let Some(t) = task_id.and_then(|id| s.tasks.iter_mut().find(|t| t.id == id)) {
                if t.status == TaskStatus::Planned {
                    t.status = TaskStatus::InProgress;
                }
            }
            s.sessions.push(Session {
                id: SessionId::from(new_id()),
                day_id,
                kind,
                task_id,
                started_at: now,
                ended_at: None,
                limit_sec,
                end_reason: None,
            });
        }
        Ok(self.status_at(now))
    }

    fn end(
        &self,
        now: DateTime<Utc>,
        expected: SessionKind,
        reason: EndReason,
    ) -> ApiResult<SessionDto> {
        let mut s = self.s.borrow_mut();
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
        let a = a.clone();
        Ok(Self::session_dto(&s, &a, now))
    }

    fn session_dto(s: &State, x: &Session, now: DateTime<Utc>) -> SessionDto {
        SessionDto {
            id: x.id,
            day_id: x.day_id,
            kind: x.kind,
            task_id: x.task_id,
            task_title: x
                .task_id
                .and_then(|id| s.tasks.iter().find(|t| t.id == id))
                .map(|t| t.title.clone()),
            started_at: x.started_at,
            ended_at: x.ended_at,
            limit_sec: x.limit_sec,
            end_reason: x.end_reason,
            duration_sec: x.duration(now),
        }
    }

    fn task_dto(s: &State, t: &Task) -> TaskDto {
        let mut children: Vec<&Task> = s
            .tasks
            .iter()
            .filter(|c| c.parent_id == Some(t.id))
            .collect();
        children.sort_by_key(|c| c.sort_order);
        TaskDto {
            id: t.id,
            parent_id: t.parent_id,
            title: t.title.clone(),
            notes: t.notes.clone(),
            status: t.status,
            planned_for: t.planned_for,
            estimate_intervals: t.estimate_intervals,
            sort_order: t.sort_order,
            created_at: t.created_at,
            done_at: t.done_at,
            external_url: t.external_url.clone(),
            children: children.into_iter().map(|c| Self::task_dto(s, c)).collect(),
        }
    }

    fn insert_task(
        s: &mut State,
        new: NewTask,
        url: Option<String>,
        now: DateTime<Utc>,
    ) -> ApiResult<TaskId> {
        let title = new.title.trim().to_string();
        if title.is_empty() {
            return Err(ApiError::Validation("пустое название".into()));
        }
        if let Some(p) = new.parent_id {
            let parent = s
                .tasks
                .iter()
                .find(|t| t.id == p)
                .ok_or(ApiError::ItemNotFound)?;
            if parent.parent_id.is_some() {
                return Err(ApiError::Validation(
                    "у пункта не бывает своих пунктов: два уровня".into(),
                ));
            }
        }
        let id = TaskId::from(new_id());
        let sort_order = s
            .tasks
            .iter()
            .filter(|t| t.parent_id == new.parent_id)
            .count() as i64;
        s.tasks.push(Task {
            id,
            parent_id: new.parent_id,
            title,
            notes: new.notes,
            status: TaskStatus::Planned,
            planned_for: new.planned_for,
            estimate_intervals: new.estimate_intervals,
            sort_order,
            created_at: now,
            done_at: None,
            external_url: url,
        });
        Ok(id)
    }

    fn tree(&self) -> Vec<TaskDto> {
        let s = self.s.borrow();
        let mut roots: Vec<&Task> = s.tasks.iter().filter(|t| t.parent_id.is_none()).collect();
        roots.sort_by_key(|t| t.sort_order);
        roots.into_iter().map(|t| Self::task_dto(&s, t)).collect()
    }

    fn find_dto(&self, id: TaskId) -> ApiResult<TaskDto> {
        let s = self.s.borrow();
        let t = s
            .tasks
            .iter()
            .find(|t| t.id == id)
            .ok_or(ApiError::ItemNotFound)?;
        Ok(Self::task_dto(&s, t))
    }

    fn tick_now(&self) -> DateTime<Utc> {
        let now = self.now();
        self.tick(now);
        now
    }
}

#[async_trait(?Send)]
impl FocusApi for MockApi {
    async fn get_status(&self) -> ApiResult<StatusDto> {
        let now = self.tick_now();
        Ok(self.status_at(now))
    }

    async fn open_day(&self) -> ApiResult<StatusDto> {
        let now = self.tick_now();
        {
            let mut s = self.s.borrow_mut();
            let today = local_date(now);
            match s.days.last_mut().filter(|d| d.date == today) {
                Some(d) => d.closed_at = None,
                None => {
                    Self::ensure_day(&mut s, now)?;
                }
            }
        }
        Ok(self.status_at(now))
    }

    async fn close_day(&self) -> ApiResult<StatusDto> {
        let now = self.tick_now();
        if self.status_at(now).active.is_some() {
            return Err(ApiError::SessionAlreadyActive);
        }
        {
            let mut s = self.s.borrow_mut();
            let today = local_date(now);
            if let Some(d) = s.days.last_mut().filter(|d| d.date == today) {
                if d.closed_at.is_none() {
                    d.closed_at = Some(now);
                }
            }
        }
        Ok(self.status_at(now))
    }

    async fn get_day(&self, date: NaiveDate) -> ApiResult<DayDto> {
        let now = self.tick_now();
        let s = self.s.borrow();
        let Some(day) = s.days.iter().rev().find(|d| d.date == date).cloned() else {
            return Ok(DayDto {
                date,
                day: None,
                sessions: vec![],
                lost: vec![],
                totals: DayTotalsDto {
                    target_sec: i64::from(s.settings.daily_target_min) * 60,
                    ..DayTotalsDto::default()
                },
            });
        };
        let mut list: Vec<&Session> = s.sessions.iter().filter(|x| x.day_id == day.id).collect();
        list.sort_by_key(|x| x.started_at);
        let sessions: Vec<SessionDto> =
            list.iter().map(|x| Self::session_dto(&s, x, now)).collect();

        // Потерянное время: промежутки без сессий внутри открытого дня.
        let mut lost = Vec::new();
        let mut cursor = day.opened_at;
        let mut any_active = false;
        for x in &list {
            if x.started_at > cursor {
                lost.push(TimeRange {
                    start: cursor,
                    end: x.started_at,
                });
            }
            any_active |= x.ended_at.is_none();
            cursor = cursor.max(x.ended_at.unwrap_or(now));
        }
        let day_end = day.closed_at.unwrap_or(now);
        if !any_active && cursor < day_end {
            lost.push(TimeRange {
                start: cursor,
                end: day_end,
            });
        }

        let sum = |k: SessionKind| -> i64 {
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
        let lost_sec = lost.iter().map(|r| (r.end - r.start).num_seconds()).sum();
        Ok(DayDto {
            date,
            totals: DayTotalsDto {
                work_sec,
                planning_sec,
                rest_sec,
                lost_sec,
                worked_sec: work_sec + planning_sec,
                target_sec: day.target_sec,
            },
            day: Some(day),
            sessions,
            lost,
        })
    }

    async fn list_tasks(&self) -> ApiResult<Vec<TaskDto>> {
        Ok(self.tree())
    }

    async fn create_task(&self, task: NewTask) -> ApiResult<TaskDto> {
        let now = self.now();
        let id = Self::insert_task(&mut self.s.borrow_mut(), task, None, now)?;
        self.find_dto(id)
    }

    async fn update_task(&self, id: TaskId, patch: TaskPatch) -> ApiResult<TaskDto> {
        let now = self.now();
        {
            let mut s = self.s.borrow_mut();
            let t = s
                .tasks
                .iter_mut()
                .find(|t| t.id == id)
                .ok_or(ApiError::ItemNotFound)?;
            if let Some(title) = patch.title {
                let title = title.trim().to_string();
                if title.is_empty() {
                    return Err(ApiError::Validation("пустое название".into()));
                }
                t.title = title;
            }
            if let Some(notes) = patch.notes {
                t.notes = notes;
            }
            if let Some(status) = patch.status {
                t.done_at = (status == TaskStatus::Done).then_some(t.done_at.unwrap_or(now));
                t.status = status;
            }
            if let Some(p) = patch.planned_for {
                t.planned_for = p;
            }
            if let Some(e) = patch.estimate_intervals {
                t.estimate_intervals = e;
            }
            if let Some(o) = patch.sort_order {
                t.sort_order = o;
            }
        }
        self.find_dto(id)
    }

    async fn start_work(&self, item_id: TaskId) -> ApiResult<StatusDto> {
        let now = self.tick_now();
        self.start(now, SessionKind::Work, Some(item_id))
    }

    async fn start_planning(&self) -> ApiResult<StatusDto> {
        let now = self.tick_now();
        self.start(now, SessionKind::Planning, None)
    }

    async fn start_rest(&self) -> ApiResult<StatusDto> {
        let now = self.tick_now();
        self.start(now, SessionKind::Rest, None)
    }

    async fn done(&self, next: DoneNext) -> ApiResult<SessionEndResult> {
        let now = self.tick_now();
        let ended = self.end(now, SessionKind::Work, EndReason::TaskDone)?;
        {
            let mut s = self.s.borrow_mut();
            if let Some(t) = s.tasks.iter_mut().find(|t| Some(t.id) == ended.task_id) {
                t.status = TaskStatus::Done;
                t.done_at = Some(now);
            }
        }
        let status = match next {
            DoneNext::Rest => self.start(now, SessionKind::Rest, None)?,
            DoneNext::Planning => self.start(now, SessionKind::Planning, None)?,
        };
        Ok(SessionEndResult { ended, status })
    }

    async fn interrupt(&self) -> ApiResult<SessionEndResult> {
        let now = self.tick_now();
        let ended = self.end(now, SessionKind::Work, EndReason::Interrupted)?;
        Ok(SessionEndResult {
            ended,
            status: self.status_at(now),
        })
    }

    async fn stop(&self) -> ApiResult<SessionEndResult> {
        let now = self.tick_now();
        let ended = self.end(now, SessionKind::Planning, EndReason::Stopped)?;
        Ok(SessionEndResult {
            ended,
            status: self.status_at(now),
        })
    }

    async fn import(&self, url: String) -> ApiResult<ImportResult> {
        let now = self.now();
        let key = jira_key(&url).ok_or_else(|| {
            ApiError::Validation("в ссылке нет ключа задачи Jira (вида PROJ-123)".into())
        })?;
        let existing = {
            let s = self.s.borrow();
            s.tasks
                .iter()
                .find(|t| {
                    t.parent_id.is_none()
                        && t.external_url.as_deref().and_then(jira_key).as_deref()
                            == Some(key.as_str())
                })
                .map(|t| t.id)
        };
        if let Some(id) = existing {
            return Ok(ImportResult {
                source: "jira".into(),
                created: false,
                added_items: 0,
                task: self.find_dto(id)?,
            });
        }
        let link = if url.trim().starts_with("http") {
            url.trim().to_string()
        } else {
            format!("https://jira.example.com/browse/{key}")
        };
        let items = [
            "Описать контракт API",
            "Покрыть сценарии оплаты тестами",
            "Обновить README",
        ];
        let id = {
            let mut s = self.s.borrow_mut();
            let id = Self::insert_task(
                &mut s,
                NewTask::new("Интеграция с биллингом"),
                Some(link),
                now,
            )?;
            for item in items {
                Self::insert_task(&mut s, NewTask::child(id, item), None, now)?;
            }
            id
        };
        Ok(ImportResult {
            source: "jira".into(),
            created: true,
            added_items: items.len() as u32,
            task: self.find_dto(id)?,
        })
    }

    async fn get_settings(&self) -> ApiResult<SettingsDto> {
        Ok(self.s.borrow().settings.clone())
    }

    async fn update_settings(&self, settings: SettingsDto) -> ApiResult<SettingsDto> {
        if settings.work_min == 0 || settings.rest_min == 0 {
            return Err(ApiError::Validation(
                "длительность — минимум 1 минута".into(),
            ));
        }
        self.s.borrow_mut().settings = settings.clone();
        Ok(settings)
    }

    async fn plugin_status(&self) -> ApiResult<Vec<PluginStatusDto>> {
        Ok(vec![PluginStatusDto {
            id: "jira".into(),
            enabled: true,
            healthy: true,
            outbox_pending: 0,
            last_error: None,
            last_error_at: None,
        }])
    }
}

#[cfg(test)]
mod tests {
    use std::future::Future;
    use std::pin::pin;
    use std::task::{Context, Poll, Waker};

    use focus_api_contract_tests::{assert_all_passed, run_all, run_all_with_clock, TestClock};
    use focus_api_types::chrono::{Duration, TimeZone, Utc};
    use focus_api_types::FocusApi;

    use super::*;

    /// Future мока готовы сразу — исполнитель не нужен.
    fn block_on<F: Future>(fut: F) -> F::Output {
        let mut fut = pin!(fut);
        let mut cx = Context::from_waker(Waker::noop());
        loop {
            if let Poll::Ready(v) = fut.as_mut().poll(&mut cx) {
                return v;
            }
        }
    }

    struct Clocked<'a>(&'a MockApi);
    impl TestClock for Clocked<'_> {
        fn advance(&self, by: Duration) {
            self.0.advance(by);
        }
    }

    fn noon() -> DateTime<Utc> {
        Local
            .with_ymd_and_hms(2026, 10, 24, 12, 0, 0)
            .earliest()
            .map(|t| t.with_timezone(&Utc))
            .unwrap_or_else(|| Utc.with_ymd_and_hms(2026, 10, 24, 12, 0, 0).unwrap())
    }

    #[test]
    fn contract_scenarios_pass_realtime() {
        let api = MockApi::realtime();
        let results = block_on(run_all(&api));
        assert_eq!(results.len(), 10);
        assert_all_passed(&results).unwrap_or_else(|e| panic!("{e}"));
    }

    #[test]
    fn contract_scenarios_pass_with_clock() {
        let api = MockApi::frozen(noon());
        let results = block_on(run_all_with_clock(&api, &Clocked(&api)));
        assert_all_passed(&results).unwrap_or_else(|e| panic!("{e}"));
    }

    #[test]
    fn work_limit_turns_into_rest_then_idle() {
        let api = MockApi::frozen(noon());
        let item = block_on(api.create_task(NewTask::new("пункт"))).unwrap();
        block_on(api.start_work(item.id)).unwrap();
        api.advance(Duration::minutes(55));
        let st = block_on(api.get_status()).unwrap();
        assert!(matches!(st.active, Some(ActiveSession::Rest { .. })));
        assert_eq!(st.worked_today_sec, 50 * 60);
        api.advance(Duration::minutes(10));
        let st = block_on(api.get_status()).unwrap();
        assert_eq!(st.active, None);
        assert_eq!(st.idle_since, Some(noon() + Duration::minutes(60)));
        let day = block_on(api.get_day(st.day.unwrap().date)).unwrap();
        assert_eq!(day.lost.len(), 1);
        assert_eq!(day.totals.lost_sec, 5 * 60);
        assert_eq!(day.sessions[0].end_reason, Some(EndReason::Limit));
    }

    #[test]
    fn demo_scenarios_have_expected_state() {
        for (sc, kind) in [
            (MockScenario::Work, Some(SessionKind::Work)),
            (MockScenario::Rest, Some(SessionKind::Rest)),
            (MockScenario::Planning, Some(SessionKind::Planning)),
            (MockScenario::Idle, None),
        ] {
            let api = MockApi::demo(sc, 0.0);
            let st = block_on(api.get_status()).unwrap();
            assert_eq!(st.active.as_ref().map(|a| a.kind()), kind, "{sc:?}");
            let day = block_on(api.get_day(st.day.unwrap().date)).unwrap();
            assert!(day.sessions.len() >= 15);
            assert!(!day.lost.is_empty());
        }
        let api = MockApi::demo(MockScenario::Work, 0.0);
        let st = block_on(api.get_status()).unwrap();
        assert_eq!(st.worked_today_sec, 5 * 3600 + 20 * 60);
    }

    #[test]
    fn import_creates_then_reuses() {
        let api = MockApi::frozen(noon());
        let r = block_on(api.import("https://jira.example.com/browse/BILL-42".into())).unwrap();
        assert!(r.created);
        assert_eq!(r.added_items, 3);
        let again = block_on(api.import("BILL-42".into())).unwrap();
        assert!(!again.created);
        assert_eq!(again.task.id, r.task.id);
        assert!(block_on(api.import("просто текст".into())).is_err());
    }

    #[test]
    fn jira_key_parsing() {
        assert_eq!(
            jira_key("https://x/browse/PROJ-123"),
            Some("PROJ-123".into())
        );
        assert_eq!(jira_key("AB2-7"), Some("AB2-7".into()));
        assert_eq!(jira_key("агент-1"), None);
        assert_eq!(jira_key("x-1"), None);
    }
}
