//! [`FocusService`] — прикладной сервис: всё поведение за контрактом
//! [`FocusApi`] поверх портов.
//!
//! Команды выполняются строго по одной (асинхронный мьютекс без привязки к
//! runtime). Перед каждой командой сервис «догоняет» время: закрывает
//! сессии, чей лимит истёк (как [`FocusService::tick`]), поэтому состояние
//! верно, даже если фоновый таймер ещё не сработал.

use std::sync::Mutex as StdMutex;

use async_trait::async_trait;
use chrono::{DateTime, NaiveDate, Utc};
use focus_api_types::{
    ActiveSession, ApiResult, DayDto, DoneNext, EndReason, FocusApi, ImportResult, ItemRef,
    NewTask, PluginStatusDto, ServerEvent, SessionDto, SessionEndResult, SessionId, SessionKind,
    SettingsDto, StatusDto, TaskDto, TaskId, TaskPatch, TaskStatus, Uuid,
};
use futures_util::lock::Mutex;

use crate::domain::{Session, Settings, Task, WorkDay};
use crate::error::{CoreError, CoreResult};
use crate::events::DomainEvent;
use crate::ports::Ports;
use crate::rules;

/// Пределы настроек (минуты).
const WORK_MIN: std::ops::RangeInclusive<u32> = 1..=240;
const REST_MIN: std::ops::RangeInclusive<u32> = 1..=120;
const TARGET_MIN: std::ops::RangeInclusive<u32> = 1..=24 * 60;
const TITLE_MAX: usize = 500;
/// Сколько событий таймера хранить до вызова `tick`.
const PENDING_MAX: usize = 64;

/// Прикладной сервис Focus Flow.
pub struct FocusService {
    p: Ports,
    /// Сериализует команды: одна команда за раз.
    op: Mutex<()>,
    /// События таймера, случившиеся при «догоне» внутри команд; отдаёт `tick`.
    pending: StdMutex<Vec<ServerEvent>>,
}

/// Что поменялось внутри команды: доменные события в outbox и события
/// таймера для клиентов.
#[derive(Default)]
struct Effects {
    domain: Vec<DomainEvent>,
    ended: Vec<SessionDto>,
}

impl FocusService {
    /// Сервис поверх набора портов.
    pub fn new(ports: Ports) -> Self {
        Self {
            p: ports,
            op: Mutex::new(()),
            pending: StdMutex::new(Vec::new()),
        }
    }

    /// Порты сервиса.
    pub fn ports(&self) -> &Ports {
        &self.p
    }

    fn now(&self) -> DateTime<Utc> {
        self.p.clock.now()
    }

    fn today(&self, now: DateTime<Utc>) -> NaiveDate {
        self.p.clock.local_date(now)
    }

    // ---------------------------------------------------------------- таймер

    /// Шаг таймера: закрывает работу по лимиту (`limit`, пункт остаётся
    /// открытым) и сразу стартует отдых; закрывает отдых, когда он кончился.
    /// Если `now` ушло далеко вперёд — проходит всю цепочку, каждая сессия
    /// закрывается ровно в свой `ends_at`.
    ///
    /// Возвращает события для клиентов: `SessionEnded` по каждой закрытой
    /// сессии (включая закрытые «догоном» внутри команд с прошлого вызова) и
    /// затем `StatusChanged`, если что-то закрылось.
    pub async fn tick(&self, now: DateTime<Utc>) -> CoreResult<Vec<ServerEvent>> {
        let _g = self.op.lock().await;
        let mut fx = Effects::default();
        self.catch_up(now, &mut fx).await?;
        self.commit(&mut fx).await?;
        let mut out = std::mem::take(&mut *lock(&self.pending));
        if !out.is_empty() {
            out.push(ServerEvent::StatusChanged(self.status_at(now).await?));
        }
        Ok(out)
    }

    /// Когда таймеру проснуться в следующий раз: `ends_at` активной работы
    /// или отдыха; `None` — ничего не истекает (нет сессии или планирование).
    pub async fn next_deadline(&self) -> CoreResult<Option<DateTime<Utc>>> {
        Ok(self.p.sessions.active().await?.and_then(|s| s.ends_at()))
    }

    /// Закрывает истёкшие сессии до момента `now`.
    async fn catch_up(&self, now: DateTime<Utc>, fx: &mut Effects) -> CoreResult<()> {
        // Цепочка конечна: работа → отдых → ничего.
        for _ in 0..4 {
            let Some(active) = self.p.sessions.active().await? else {
                return Ok(());
            };
            let Some(at) = rules::expired_at(&active, now) else {
                return Ok(());
            };
            let closed = self.close(active, at, EndReason::Limit, fx).await?;
            if closed.kind == SessionKind::Work {
                // Продолжение цепочки — в тот же день, без проверки закрытия.
                let settings = self.p.settings.load().await?;
                self.open_session(closed.day_id, SessionKind::Rest, None, at, &settings)
                    .await?;
            }
        }
        Ok(())
    }

    /// Закрыть сессию в момент `at` с причиной `reason`.
    async fn close(
        &self,
        mut s: Session,
        at: DateTime<Utc>,
        reason: EndReason,
        fx: &mut Effects,
    ) -> CoreResult<Session> {
        s.ended_at = Some(at.max(s.started_at));
        s.end_reason = Some(reason);
        self.p.sessions.save(&s).await?;
        let (item, parent) = self.item_with_parent(s.task_id).await?;
        fx.ended
            .push(session_dto(&s, at, item.as_ref().map(|t| t.title.clone())));
        fx.domain.push(DomainEvent::SessionClosed {
            session: s.clone(),
            item,
            parent,
        });
        Ok(s)
    }

    async fn open_session(
        &self,
        day_id: focus_api_types::DayId,
        kind: SessionKind,
        task_id: Option<TaskId>,
        at: DateTime<Utc>,
        settings: &Settings,
    ) -> CoreResult<Session> {
        let s = Session {
            id: SessionId::from(Uuid::now_v7()),
            day_id,
            kind,
            task_id,
            started_at: at,
            ended_at: None,
            limit_sec: settings.limit_for(kind),
            end_reason: None,
        };
        self.p.sessions.save(&s).await?;
        Ok(s)
    }

    /// Публикует доменные события и откладывает `SessionEnded` для `tick`.
    async fn commit(&self, fx: &mut Effects) -> CoreResult<()> {
        if !fx.domain.is_empty() {
            self.p.events.publish(&fx.domain).await?;
            fx.domain.clear();
        }
        if !fx.ended.is_empty() {
            let mut pending = lock(&self.pending);
            pending.extend(
                fx.ended
                    .drain(..)
                    .map(|session| ServerEvent::SessionEnded { session }),
            );
            // Если `tick` никто не вызывает (тесты, мок) — не копим бесконечно.
            let excess = pending.len().saturating_sub(PENDING_MAX);
            pending.drain(..excess);
        }
        Ok(())
    }

    /// Начало команды: блокировка + «догон» времени.
    async fn begin(&self) -> CoreResult<(futures_util::lock::MutexGuard<'_, ()>, DateTime<Utc>)> {
        let g = self.op.lock().await;
        let now = self.now();
        let mut fx = Effects::default();
        self.catch_up(now, &mut fx).await?;
        self.commit(&mut fx).await?;
        Ok((g, now))
    }

    // ---------------------------------------------------------------- статус

    /// `GET /api/status`.
    pub async fn status(&self) -> CoreResult<StatusDto> {
        let (_g, now) = self.begin().await?;
        self.status_at(now).await
    }

    /// Статус без блокировки и «догона» (вызывать внутри команды).
    ///
    /// Текущий день — день активной сессии (сессия через полночь остаётся во
    /// вчерашнем дне до конца), иначе — день сегодняшней локальной даты.
    async fn status_at(&self, now: DateTime<Utc>) -> CoreResult<StatusDto> {
        let active = self.p.sessions.active().await?;
        let day = match &active {
            Some(a) => self.p.days.get(a.day_id).await?,
            None => self.p.days.by_date(self.today(now)).await?,
        };
        let sessions = match &day {
            Some(d) => self.p.sessions.by_day(d.id).await?,
            None => Vec::new(),
        };
        let target_sec = match &day {
            Some(d) => d.target_sec,
            None => self.p.settings.load().await?.target_sec(),
        };
        let idle_since = day
            .as_ref()
            .filter(|d| d.is_open())
            .and_then(|d| rules::idle_since(d, &sessions, now));
        let active = match active {
            Some(a) => Some(self.active_dto(&a).await?),
            None => None,
        };
        Ok(StatusDto {
            server_time: now,
            day: day.as_ref().map(WorkDay::to_dto),
            active,
            worked_today_sec: rules::worked_sec(&sessions, now),
            target_sec,
            idle_since,
        })
    }

    async fn active_dto(&self, s: &Session) -> CoreResult<ActiveSession> {
        Ok(match s.kind {
            SessionKind::Work => {
                let (item, parent) = self.item_with_parent(s.task_id).await?;
                let item = item.ok_or_else(|| {
                    CoreError::Storage(format!("пункт сессии {} не найден", s.id))
                })?;
                ActiveSession::Work {
                    session_id: s.id,
                    item: ItemRef {
                        id: item.id,
                        title: item.title,
                        parent_id: item.parent_id,
                        parent_title: parent.map(|p| p.title),
                    },
                    started_at: s.started_at,
                    ends_at: s.ends_at().unwrap_or(s.started_at),
                }
            }
            SessionKind::Rest => ActiveSession::Rest {
                session_id: s.id,
                started_at: s.started_at,
                ends_at: s.ends_at().unwrap_or(s.started_at),
            },
            SessionKind::Planning => ActiveSession::Planning {
                session_id: s.id,
                started_at: s.started_at,
            },
        })
    }

    async fn item_with_parent(
        &self,
        id: Option<TaskId>,
    ) -> CoreResult<(Option<Task>, Option<Task>)> {
        let Some(id) = id else {
            return Ok((None, None));
        };
        let item = self.p.tasks.get(id).await?;
        let parent = match item.as_ref().and_then(|t| t.parent_id) {
            Some(p) => self.p.tasks.get(p).await?,
            None => None,
        };
        Ok((item, parent))
    }

    // ---------------------------------------------------------------- день

    /// `POST /api/day/open`: открыть сегодняшний день; закрытый — открыть снова.
    pub async fn open_day(&self) -> CoreResult<StatusDto> {
        let (_g, now) = self.begin().await?;
        let date = self.today(now);
        match self.p.days.by_date(date).await? {
            Some(mut d) => {
                if !d.is_open() {
                    d.closed_at = None;
                    self.p.days.save(&d).await?;
                }
            }
            None => {
                self.create_day(date, now).await?;
            }
        }
        self.status_at(now).await
    }

    /// `POST /api/day/close`: закрыть текущий день. При активной сессии —
    /// `SessionAlreadyActive`; если день не открывался — `Validation`.
    pub async fn close_day(&self) -> CoreResult<StatusDto> {
        let (_g, now) = self.begin().await?;
        if self.p.sessions.active().await?.is_some() {
            return Err(CoreError::SessionAlreadyActive);
        }
        let mut day = self
            .p
            .days
            .by_date(self.today(now))
            .await?
            .ok_or_else(|| CoreError::Validation("сегодня день не открывался".into()))?;
        if day.is_open() {
            day.closed_at = Some(now.max(day.opened_at));
            self.p.days.save(&day).await?;
        }
        self.status_at(now).await
    }

    async fn create_day(&self, date: NaiveDate, now: DateTime<Utc>) -> CoreResult<WorkDay> {
        let settings = self.p.settings.load().await?;
        let day = WorkDay {
            id: focus_api_types::DayId::from(Uuid::now_v7()),
            date,
            opened_at: now,
            closed_at: None,
            target_sec: settings.target_sec(),
        };
        self.p.days.save(&day).await?;
        Ok(day)
    }

    /// День для новой сессии, стартующей по команде: сегодняшний, открывается
    /// автоматически; закрытый — `DayClosed`.
    async fn day_for_start(&self, now: DateTime<Utc>) -> CoreResult<WorkDay> {
        match self.p.days.by_date(self.today(now)).await? {
            Some(d) if !d.is_open() => Err(CoreError::DayClosed),
            Some(d) => Ok(d),
            None => self.create_day(self.today(now), now).await,
        }
    }

    /// `GET /api/days/{date}`: сессии, потерянное время и итоги дня.
    pub async fn day(&self, date: NaiveDate) -> CoreResult<DayDto> {
        let (_g, now) = self.begin().await?;
        let Some(day) = self.p.days.by_date(date).await? else {
            return Ok(DayDto {
                date,
                day: None,
                sessions: Vec::new(),
                lost: Vec::new(),
                totals: focus_api_types::DayTotalsDto {
                    target_sec: self.p.settings.load().await?.target_sec(),
                    ..Default::default()
                },
            });
        };
        let sessions = self.p.sessions.by_day(day.id).await?;
        // Хвост потерянного времени: до закрытия дня; у открытого сегодняшнего
        // дня — до «сейчас»; у прошлого незакрытого — не считается.
        let until = match day.closed_at {
            Some(c) => Some(c),
            None if day.date == self.today(now) => Some(now),
            None => None,
        };
        let lost = rules::lost_ranges(&day, &sessions, now, until);
        let totals = rules::totals(&sessions, &lost, now, day.target_sec);
        let mut dtos = Vec::with_capacity(sessions.len());
        for s in &sessions {
            let title = match s.task_id {
                Some(id) => self.p.tasks.get(id).await?.map(|t| t.title),
                None => None,
            };
            dtos.push(session_dto(s, now, title));
        }
        Ok(DayDto {
            date,
            day: Some(day.to_dto()),
            sessions: dtos,
            lost,
            totals,
        })
    }

    // ---------------------------------------------------------------- задачи

    /// `GET /api/tasks`: дерево задач.
    pub async fn list_tasks(&self) -> CoreResult<Vec<TaskDto>> {
        let _g = self.op.lock().await;
        Ok(rules::build_tree(&self.p.tasks.list().await?))
    }

    async fn subtree(&self, id: TaskId) -> CoreResult<TaskDto> {
        let task = self.p.tasks.get(id).await?.ok_or(CoreError::ItemNotFound)?;
        let mut dto = task.to_dto();
        let mut children = self.p.tasks.children(Some(id)).await?;
        children.sort_by_key(|t| (t.sort_order, t.created_at, t.id));
        dto.children = children.iter().map(Task::to_dto).collect();
        Ok(dto)
    }

    async fn next_sort_order(&self, parent: Option<TaskId>) -> CoreResult<i64> {
        Ok(self
            .p
            .tasks
            .children(parent)
            .await?
            .iter()
            .map(|t| t.sort_order)
            .max()
            .map_or(0, |m| m + 1))
    }

    /// `POST /api/tasks`: задача верхнего уровня или пункт внутри неё.
    /// Глубина — два уровня; к пункту в активной работе детей не добавить.
    pub async fn create_task(&self, new: NewTask) -> CoreResult<TaskDto> {
        let (_g, now) = self.begin().await?;
        let title = clean_title(&new.title)?;
        let parent = match new.parent_id {
            Some(pid) => {
                let parent = self
                    .p
                    .tasks
                    .get(pid)
                    .await?
                    .ok_or(CoreError::ItemNotFound)?;
                if parent.parent_id.is_some() {
                    return Err(CoreError::Validation(
                        "дерево задач — два уровня: у пункта не бывает пунктов".into(),
                    ));
                }
                if let Some(a) = self.p.sessions.active().await?
                    && a.task_id == Some(pid)
                {
                    return Err(CoreError::Validation(
                        "нельзя добавить пункт к узлу, над которым идёт работа".into(),
                    ));
                }
                Some(parent)
            }
            None => None,
        };
        let task = Task {
            id: TaskId::from(Uuid::now_v7()),
            parent_id: new.parent_id,
            title,
            notes: new.notes,
            status: TaskStatus::Planned,
            planned_for: new.planned_for,
            estimate_intervals: new.estimate_intervals,
            sort_order: self.next_sort_order(new.parent_id).await?,
            created_at: now,
            done_at: None,
            external_url: None,
        };
        self.p.tasks.save(&task).await?;
        self.p
            .events
            .publish(&[DomainEvent::ItemCreated {
                task: task.clone(),
                parent,
            }])
            .await?;
        Ok(task.to_dto())
    }

    /// `PATCH /api/tasks/{id}`: частичная правка. Переход в `done` публикует
    /// `ItemCompleted`.
    pub async fn update_task(&self, id: TaskId, patch: TaskPatch) -> CoreResult<TaskDto> {
        let (_g, now) = self.begin().await?;
        let mut task = self.p.tasks.get(id).await?.ok_or(CoreError::ItemNotFound)?;
        if let Some(title) = &patch.title {
            task.title = clean_title(title)?;
        }
        if let Some(notes) = patch.notes {
            task.notes = notes;
        }
        if let Some(planned_for) = patch.planned_for {
            task.planned_for = planned_for;
        }
        if let Some(estimate) = patch.estimate_intervals {
            task.estimate_intervals = estimate;
        }
        if let Some(order) = patch.sort_order {
            task.sort_order = order;
        }
        let became_done = match patch.status {
            Some(status) => task.set_status(status, now),
            None => false,
        };
        self.p.tasks.save(&task).await?;
        if became_done {
            let parent = match task.parent_id {
                Some(p) => self.p.tasks.get(p).await?,
                None => None,
            };
            self.p
                .events
                .publish(&[DomainEvent::ItemCompleted {
                    task: task.clone(),
                    parent,
                }])
                .await?;
        }
        self.subtree(id).await
    }

    // ---------------------------------------------------------------- сессии

    /// `POST /api/sessions/work`: начать работу над пунктом. Автооткрывает
    /// день; активное планирование закрывается с `switched`.
    pub async fn start_work(&self, item_id: TaskId) -> CoreResult<StatusDto> {
        self.start(SessionKind::Work, Some(item_id)).await
    }

    /// `POST /api/sessions/planning`: начать планирование.
    pub async fn start_planning(&self) -> CoreResult<StatusDto> {
        self.start(SessionKind::Planning, None).await
    }

    /// `POST /api/sessions/rest`: начать отдых. Активное планирование
    /// закрывается с `switched`.
    pub async fn start_rest(&self) -> CoreResult<StatusDto> {
        self.start(SessionKind::Rest, None).await
    }

    async fn start(&self, kind: SessionKind, item_id: Option<TaskId>) -> CoreResult<StatusDto> {
        let (_g, now) = self.begin().await?;
        // 1. Конфликт с активной сессией: прервать стартом можно только
        //    планирование, и только не другим планированием.
        let active = self.p.sessions.active().await?;
        if let Some(a) = &active
            && (a.kind != SessionKind::Planning || kind == SessionKind::Planning)
        {
            return Err(CoreError::SessionAlreadyActive);
        }
        // 2. Пункт.
        let item = match item_id {
            Some(id) => Some(self.check_item(id).await?),
            None => None,
        };
        // 3. День (автооткрытие).
        let day = self.day_for_start(now).await?;

        let mut fx = Effects::default();
        if let Some(a) = active {
            self.close(a, now, EndReason::Switched, &mut fx).await?;
        }
        if let Some(mut item) = item
            && item.status == TaskStatus::Planned
        {
            item.status = TaskStatus::InProgress;
            self.p.tasks.save(&item).await?;
        }
        let settings = self.p.settings.load().await?;
        self.open_session(day.id, kind, item_id, now, &settings)
            .await?;
        self.commit(&mut fx).await?;
        self.status_at(now).await
    }

    /// Пункт, который можно взять в работу: существует, без детей, не закрыт.
    async fn check_item(&self, id: TaskId) -> CoreResult<Task> {
        let item = self.p.tasks.get(id).await?.ok_or(CoreError::ItemNotFound)?;
        if !self.p.tasks.children(Some(id)).await?.is_empty() {
            return Err(CoreError::ItemRequired);
        }
        if matches!(item.status, TaskStatus::Done | TaskStatus::Cancelled) {
            return Err(CoreError::Validation(
                "пункт уже закрыт (done/cancelled) — сначала верните его в planned".into(),
            ));
        }
        Ok(item)
    }

    /// Активная сессия нужного вида.
    async fn active_of(&self, expected: SessionKind) -> CoreResult<Session> {
        let a = self
            .p
            .sessions
            .active()
            .await?
            .ok_or(CoreError::NoActiveSession)?;
        if a.kind != expected {
            return Err(CoreError::WrongSessionKind {
                expected,
                actual: a.kind,
            });
        }
        Ok(a)
    }

    /// `POST /api/sessions/active/done`: закрыть работу (`task_done`), пункт →
    /// `done`, сразу начать `next` в тот же момент и в тот же день.
    pub async fn done(&self, next: DoneNext) -> CoreResult<SessionEndResult> {
        let (_g, now) = self.begin().await?;
        let work = self.active_of(SessionKind::Work).await?;
        let mut fx = Effects::default();
        let ended = self.close(work, now, EndReason::TaskDone, &mut fx).await?;
        let (item, parent) = self.item_with_parent(ended.task_id).await?;
        if let Some(mut item) = item
            && item.set_status(TaskStatus::Done, now)
        {
            self.p.tasks.save(&item).await?;
            fx.domain
                .push(DomainEvent::ItemCompleted { task: item, parent });
        }
        let kind = match next {
            DoneNext::Rest => SessionKind::Rest,
            DoneNext::Planning => SessionKind::Planning,
        };
        let settings = self.p.settings.load().await?;
        self.open_session(ended.day_id, kind, None, now, &settings)
            .await?;
        self.finish(ended, now, fx).await
    }

    /// `POST /api/sessions/active/interrupt`: «Прервать» работу. Пункт
    /// остаётся открытым.
    pub async fn interrupt(&self) -> CoreResult<SessionEndResult> {
        let (_g, now) = self.begin().await?;
        let work = self.active_of(SessionKind::Work).await?;
        let mut fx = Effects::default();
        let ended = self
            .close(work, now, EndReason::Interrupted, &mut fx)
            .await?;
        self.finish(ended, now, fx).await
    }

    /// `POST /api/sessions/active/stop`: «Стоп» планирования.
    pub async fn stop(&self) -> CoreResult<SessionEndResult> {
        let (_g, now) = self.begin().await?;
        let planning = self.active_of(SessionKind::Planning).await?;
        let mut fx = Effects::default();
        let ended = self
            .close(planning, now, EndReason::Stopped, &mut fx)
            .await?;
        self.finish(ended, now, fx).await
    }

    async fn finish(
        &self,
        ended: Session,
        now: DateTime<Utc>,
        mut fx: Effects,
    ) -> CoreResult<SessionEndResult> {
        let ended = fx
            .ended
            .pop()
            .unwrap_or_else(|| session_dto(&ended, now, None));
        // Закрытие по команде клиент видит в ответе; в `tick` уходят только
        // сессии, закрытые таймером.
        fx.ended.clear();
        self.commit(&mut fx).await?;
        Ok(SessionEndResult {
            ended,
            status: self.status_at(now).await?,
        })
    }

    // ---------------------------------------------------------------- импорт

    /// `POST /api/import`: задача из внешнего источника. Повторный импорт той
    /// же ссылки не дублирует задачу, а догружает пункты с новыми названиями.
    pub async fn import(&self, url: &str) -> CoreResult<ImportResult> {
        let url = url.trim();
        if url.is_empty() {
            return Err(CoreError::Validation("пустая ссылка".into()));
        }
        // Сеть — вне блокировки: импорт может идти долго.
        let imported = self.p.sources.import(url).await?;
        let (_g, now) = self.begin().await?;
        // Название уже существующей задачи не трогаем: его мог править пользователь.
        let (task, created) = match self.p.tasks.by_external_url(&imported.url).await? {
            Some(t) => (t, false),
            None => {
                let t = Task {
                    id: TaskId::from(Uuid::now_v7()),
                    parent_id: None,
                    title: clean_title(&imported.title)?,
                    notes: imported.notes.clone(),
                    status: TaskStatus::Planned,
                    planned_for: None,
                    estimate_intervals: None,
                    sort_order: self.next_sort_order(None).await?,
                    created_at: now,
                    done_at: None,
                    external_url: Some(imported.url.clone()),
                };
                self.p.tasks.save(&t).await?;
                (t, true)
            }
        };
        let existing: Vec<String> = self
            .p
            .tasks
            .children(Some(task.id))
            .await?
            .into_iter()
            .map(|c| c.title)
            .collect();
        let mut order = self.next_sort_order(Some(task.id)).await?;
        let mut added = 0u32;
        for item in &imported.items {
            let Ok(title) = clean_title(&item.title) else {
                continue;
            };
            if existing.contains(&title) {
                continue;
            }
            let mut child = Task {
                id: TaskId::from(Uuid::now_v7()),
                parent_id: Some(task.id),
                title,
                notes: None,
                status: TaskStatus::Planned,
                planned_for: None,
                estimate_intervals: None,
                sort_order: order,
                created_at: now,
                done_at: None,
                external_url: None,
            };
            child.set_status(item.status, now);
            self.p.tasks.save(&child).await?;
            order += 1;
            added += 1;
        }
        self.p
            .events
            .publish(&[DomainEvent::TaskImported {
                source: imported.source.clone(),
                task: task.clone(),
                created,
            }])
            .await?;
        Ok(ImportResult {
            source: imported.source,
            created,
            added_items: added,
            task: self.subtree(task.id).await?,
        })
    }

    // ---------------------------------------------------------------- настройки

    /// `GET /api/settings`.
    pub async fn settings(&self) -> CoreResult<SettingsDto> {
        Ok(self.p.settings.load().await?.into())
    }

    /// `PUT /api/settings`: заменить настройки. Активная сессия и уже
    /// открытый день сохраняют свои лимит и норму.
    pub async fn update_settings(&self, dto: SettingsDto) -> CoreResult<SettingsDto> {
        let _g = self.op.lock().await;
        check_range("work_min", dto.work_min, WORK_MIN)?;
        check_range("rest_min", dto.rest_min, REST_MIN)?;
        check_range("daily_target_min", dto.daily_target_min, TARGET_MIN)?;
        let settings = Settings::from(dto);
        self.p.settings.save(&settings).await?;
        Ok(settings.into())
    }

    /// `GET /api/plugins/status`. Плагинов в ядре нет — пусто; настоящее
    /// состояние собирает `focus-server` (шаг 5).
    pub async fn plugin_status(&self) -> CoreResult<Vec<PluginStatusDto>> {
        Ok(Vec::new())
    }
}

fn lock<T>(m: &StdMutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn clean_title(title: &str) -> CoreResult<String> {
    let t = title.trim();
    if t.is_empty() {
        return Err(CoreError::Validation("пустое название".into()));
    }
    if t.chars().count() > TITLE_MAX {
        return Err(CoreError::Validation(format!(
            "название длиннее {TITLE_MAX} символов"
        )));
    }
    Ok(t.to_owned())
}

fn check_range(name: &str, v: u32, range: std::ops::RangeInclusive<u32>) -> CoreResult<()> {
    if range.contains(&v) {
        Ok(())
    } else {
        Err(CoreError::Validation(format!(
            "{name} = {v}: допустимо {}..={}",
            range.start(),
            range.end()
        )))
    }
}

/// Запись о сессии для API.
pub fn session_dto(s: &Session, now: DateTime<Utc>, task_title: Option<String>) -> SessionDto {
    SessionDto {
        id: s.id,
        day_id: s.day_id,
        kind: s.kind,
        task_id: s.task_id,
        task_title,
        started_at: s.started_at,
        ended_at: s.ended_at,
        limit_sec: s.limit_sec,
        end_reason: s.end_reason,
        duration_sec: s.duration_sec(now),
    }
}

/// Тонкая обёртка контракта: каждая операция — inherent-метод (их future
/// `Send`, ими пользуется сервер), ошибки ядра → [`focus_api_types::ApiError`].
#[async_trait(?Send)]
impl FocusApi for FocusService {
    async fn get_status(&self) -> ApiResult<StatusDto> {
        Ok(self.status().await?)
    }
    async fn open_day(&self) -> ApiResult<StatusDto> {
        Ok(FocusService::open_day(self).await?)
    }
    async fn close_day(&self) -> ApiResult<StatusDto> {
        Ok(FocusService::close_day(self).await?)
    }
    async fn get_day(&self, date: NaiveDate) -> ApiResult<DayDto> {
        Ok(self.day(date).await?)
    }
    async fn list_tasks(&self) -> ApiResult<Vec<TaskDto>> {
        Ok(FocusService::list_tasks(self).await?)
    }
    async fn create_task(&self, task: NewTask) -> ApiResult<TaskDto> {
        Ok(FocusService::create_task(self, task).await?)
    }
    async fn update_task(&self, id: TaskId, patch: TaskPatch) -> ApiResult<TaskDto> {
        Ok(FocusService::update_task(self, id, patch).await?)
    }
    async fn start_work(&self, item_id: TaskId) -> ApiResult<StatusDto> {
        Ok(FocusService::start_work(self, item_id).await?)
    }
    async fn start_planning(&self) -> ApiResult<StatusDto> {
        Ok(FocusService::start_planning(self).await?)
    }
    async fn start_rest(&self) -> ApiResult<StatusDto> {
        Ok(FocusService::start_rest(self).await?)
    }
    async fn done(&self, next: DoneNext) -> ApiResult<SessionEndResult> {
        Ok(FocusService::done(self, next).await?)
    }
    async fn interrupt(&self) -> ApiResult<SessionEndResult> {
        Ok(FocusService::interrupt(self).await?)
    }
    async fn stop(&self) -> ApiResult<SessionEndResult> {
        Ok(FocusService::stop(self).await?)
    }
    async fn import(&self, url: String) -> ApiResult<ImportResult> {
        Ok(FocusService::import(self, &url).await?)
    }
    async fn get_settings(&self) -> ApiResult<SettingsDto> {
        Ok(self.settings().await?)
    }
    async fn update_settings(&self, settings: SettingsDto) -> ApiResult<SettingsDto> {
        Ok(FocusService::update_settings(self, settings).await?)
    }
    async fn plugin_status(&self) -> ApiResult<Vec<PluginStatusDto>> {
        Ok(FocusService::plugin_status(self).await?)
    }
}
