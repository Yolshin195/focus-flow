//! Реализации всех портов в памяти: для тестов ядра и, при желании, мока.
//! Только `std::sync::Mutex` — без tokio и других runtime.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard};

use async_trait::async_trait;
use chrono::NaiveDate;
use focus_api_types::{DayId, SessionId, TaskId};

use crate::clock::Clock;
use crate::domain::{Session, Settings, Task, WorkDay};
use crate::error::CoreResult;
use crate::events::DomainEvent;
use crate::ports::{
    DayRepo, EventSink, NoTaskSources, Ports, SessionRepo, SettingsRepo, TaskRepo, TaskSources,
};

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    // Внутри замков нет паник; отравление не ломает данные.
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Дни в памяти.
#[derive(Debug, Default)]
pub struct MemoryDays(Mutex<BTreeMap<DayId, WorkDay>>);

#[async_trait]
impl DayRepo for MemoryDays {
    async fn get(&self, id: DayId) -> CoreResult<Option<WorkDay>> {
        Ok(lock(&self.0).get(&id).cloned())
    }

    async fn by_date(&self, date: NaiveDate) -> CoreResult<Option<WorkDay>> {
        Ok(lock(&self.0).values().find(|d| d.date == date).cloned())
    }

    async fn save(&self, day: &WorkDay) -> CoreResult<()> {
        lock(&self.0).insert(day.id, day.clone());
        Ok(())
    }
}

/// Задачи в памяти.
#[derive(Debug, Default)]
pub struct MemoryTasks(Mutex<BTreeMap<TaskId, Task>>);

#[async_trait]
impl TaskRepo for MemoryTasks {
    async fn get(&self, id: TaskId) -> CoreResult<Option<Task>> {
        Ok(lock(&self.0).get(&id).cloned())
    }

    async fn list(&self) -> CoreResult<Vec<Task>> {
        Ok(lock(&self.0).values().cloned().collect())
    }

    async fn children(&self, parent: Option<TaskId>) -> CoreResult<Vec<Task>> {
        Ok(lock(&self.0)
            .values()
            .filter(|t| t.parent_id == parent)
            .cloned()
            .collect())
    }

    async fn by_external_url(&self, url: &str) -> CoreResult<Option<Task>> {
        Ok(lock(&self.0)
            .values()
            .find(|t| t.external_url.as_deref() == Some(url))
            .cloned())
    }

    async fn save(&self, task: &Task) -> CoreResult<()> {
        lock(&self.0).insert(task.id, task.clone());
        Ok(())
    }
}

/// Сессии в памяти.
#[derive(Debug, Default)]
pub struct MemorySessions(Mutex<BTreeMap<SessionId, Session>>);

#[async_trait]
impl SessionRepo for MemorySessions {
    async fn get(&self, id: SessionId) -> CoreResult<Option<Session>> {
        Ok(lock(&self.0).get(&id).cloned())
    }

    async fn active(&self) -> CoreResult<Option<Session>> {
        Ok(lock(&self.0).values().find(|s| s.is_active()).cloned())
    }

    async fn by_day(&self, day_id: DayId) -> CoreResult<Vec<Session>> {
        let mut v: Vec<Session> = lock(&self.0)
            .values()
            .filter(|s| s.day_id == day_id)
            .cloned()
            .collect();
        v.sort_by_key(|s| s.started_at);
        Ok(v)
    }

    async fn save(&self, session: &Session) -> CoreResult<()> {
        lock(&self.0).insert(session.id, session.clone());
        Ok(())
    }
}

/// Настройки в памяти.
#[derive(Debug, Default)]
pub struct MemorySettings(Mutex<Settings>);

#[async_trait]
impl SettingsRepo for MemorySettings {
    async fn load(&self) -> CoreResult<Settings> {
        Ok(lock(&self.0).clone())
    }

    async fn save(&self, settings: &Settings) -> CoreResult<()> {
        *lock(&self.0) = settings.clone();
        Ok(())
    }
}

/// Outbox в памяти: просто копит опубликованные события.
#[derive(Debug, Default)]
pub struct MemoryOutbox(Mutex<Vec<DomainEvent>>);

impl MemoryOutbox {
    /// Все опубликованные события (копия).
    pub fn events(&self) -> Vec<DomainEvent> {
        lock(&self.0).clone()
    }

    /// Забрать опубликованные события, очистив очередь.
    pub fn take(&self) -> Vec<DomainEvent> {
        std::mem::take(&mut *lock(&self.0))
    }
}

#[async_trait]
impl EventSink for MemoryOutbox {
    async fn publish(&self, events: &[DomainEvent]) -> CoreResult<()> {
        lock(&self.0).extend_from_slice(events);
        Ok(())
    }
}

/// Все репозитории в памяти с доступом к конкретным типам (например,
/// чтобы проверить outbox в тесте).
#[derive(Debug, Default, Clone)]
pub struct MemoryStore {
    /// Дни.
    pub days: Arc<MemoryDays>,
    /// Задачи.
    pub tasks: Arc<MemoryTasks>,
    /// Сессии.
    pub sessions: Arc<MemorySessions>,
    /// Настройки.
    pub settings: Arc<MemorySettings>,
    /// Outbox.
    pub outbox: Arc<MemoryOutbox>,
}

impl MemoryStore {
    /// Пустое хранилище с настройками по умолчанию.
    pub fn new() -> Self {
        Self::default()
    }

    /// Порты поверх этого хранилища без источников задач.
    pub fn ports(&self, clock: Arc<dyn Clock>) -> Ports {
        self.ports_with_sources(clock, Arc::new(NoTaskSources))
    }

    /// Порты поверх этого хранилища с заданными источниками задач.
    pub fn ports_with_sources(
        &self,
        clock: Arc<dyn Clock>,
        sources: Arc<dyn TaskSources>,
    ) -> Ports {
        Ports {
            clock,
            days: self.days.clone(),
            tasks: self.tasks.clone(),
            sessions: self.sessions.clone(),
            settings: self.settings.clone(),
            events: self.outbox.clone(),
            sources,
        }
    }
}
