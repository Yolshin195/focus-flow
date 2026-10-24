//! Реализации всех портов в памяти: для тестов ядра и, при желании, мока.
//! Только `std::sync::Mutex` — без tokio и других runtime.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard};

use async_trait::async_trait;
use chrono::{DateTime, NaiveDate, Utc};
use focus_api_types::{DayId, SessionId, TaskId};

use crate::clock::Clock;
use crate::domain::{Session, Settings, Task, WorkDay};
use crate::error::{CoreError, CoreResult};
use crate::events::DomainEvent;
use crate::outbox::{OutboxEntry, OutboxId, OutboxStats, select_due};
use crate::ports::{
    DayRepo, EventSink, NoTaskSources, Outbox, Ports, SessionRepo, SettingsRepo, TaskRepo,
    TaskSources,
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

/// Outbox в памяти: журнал всех опубликованных событий (для тестов) и
/// записи доставки по подписчикам — как в SQLite.
#[derive(Debug, Default)]
pub struct MemoryOutbox(Mutex<OutboxState>);

#[derive(Debug, Default)]
struct OutboxState {
    log: Vec<DomainEvent>,
    subscribers: Vec<String>,
    entries: Vec<OutboxEntry>,
    next_id: OutboxId,
}

impl OutboxState {
    fn entry(&mut self, id: OutboxId) -> CoreResult<&mut OutboxEntry> {
        self.entries
            .iter_mut()
            .find(|e| e.id == id)
            .ok_or_else(|| CoreError::storage(format!("нет записи outbox {id}")))
    }
}

impl MemoryOutbox {
    /// Все опубликованные события (копия журнала).
    pub fn events(&self) -> Vec<DomainEvent> {
        lock(&self.0).log.clone()
    }

    /// Забрать опубликованные события, очистив журнал (записи доставки
    /// не трогаются).
    pub fn take(&self) -> Vec<DomainEvent> {
        std::mem::take(&mut lock(&self.0).log)
    }
}

#[async_trait]
impl EventSink for MemoryOutbox {
    async fn publish(&self, events: &[DomainEvent]) -> CoreResult<()> {
        let mut st = lock(&self.0);
        st.log.extend_from_slice(events);
        for ev in events {
            for plugin_id in st.subscribers.clone() {
                st.next_id += 1;
                let id = st.next_id;
                st.entries.push(OutboxEntry {
                    id,
                    plugin_id,
                    event: ev.clone(),
                    attempts: 0,
                    next_attempt_at: None,
                    last_error: None,
                    last_error_at: None,
                    done_at: None,
                });
            }
        }
        Ok(())
    }
}

#[async_trait]
impl Outbox for MemoryOutbox {
    async fn set_subscribers(&self, plugin_ids: &[String]) -> CoreResult<()> {
        lock(&self.0).subscribers = plugin_ids.to_vec();
        Ok(())
    }

    async fn fetch_due(&self, now: DateTime<Utc>, limit: usize) -> CoreResult<Vec<OutboxEntry>> {
        Ok(select_due(&lock(&self.0).entries, now, limit))
    }

    async fn mark_done(&self, id: OutboxId, now: DateTime<Utc>) -> CoreResult<()> {
        let mut st = lock(&self.0);
        let e = st.entry(id)?;
        e.done_at = Some(now);
        e.next_attempt_at = None;
        Ok(())
    }

    async fn mark_failed(
        &self,
        id: OutboxId,
        error: &str,
        now: DateTime<Utc>,
    ) -> CoreResult<DateTime<Utc>> {
        let mut st = lock(&self.0);
        let e = st.entry(id)?;
        let next = e.retry_at(now);
        e.attempts = e.attempts.saturating_add(1);
        e.last_error = Some(error.to_owned());
        e.last_error_at = Some(now);
        e.next_attempt_at = Some(next);
        Ok(next)
    }

    async fn stats(&self) -> CoreResult<Vec<OutboxStats>> {
        let st = lock(&self.0);
        let mut by: BTreeMap<&str, OutboxStats> = BTreeMap::new();
        for e in &st.entries {
            let s = by
                .entry(e.plugin_id.as_str())
                .or_insert_with(|| OutboxStats {
                    plugin_id: e.plugin_id.clone(),
                    pending: 0,
                    last_error: None,
                    last_error_at: None,
                });
            if e.is_pending() {
                s.pending += 1;
                if e.last_error_at.is_some() && e.last_error_at >= s.last_error_at {
                    s.last_error = e.last_error.clone();
                    s.last_error_at = e.last_error_at;
                }
            }
        }
        Ok(by.into_values().collect())
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

#[cfg(test)]
mod tests {
    use chrono::{Duration, TimeZone};
    use futures_executor::block_on;

    use super::*;

    #[test]
    fn memory_outbox_lifecycle() {
        let now = Utc.with_ymd_and_hms(2026, 10, 24, 9, 0, 0).unwrap();
        let ob = MemoryOutbox::default();
        block_on(ob.publish(&[DomainEvent::Unknown])).unwrap();
        assert!(block_on(ob.fetch_due(now, 10)).unwrap().is_empty());

        block_on(ob.set_subscribers(&["jira".into()])).unwrap();
        block_on(ob.publish(&[DomainEvent::Unknown, DomainEvent::Unknown])).unwrap();
        let due = block_on(ob.fetch_due(now, 10)).unwrap();
        assert_eq!(due.len(), 2);

        let next = block_on(ob.mark_failed(due[0].id, "сеть", now)).unwrap();
        assert_eq!(next, now + Duration::seconds(10));
        // Первая ждёт повтора — вторая того же плагина тоже ждёт.
        assert!(block_on(ob.fetch_due(now, 10)).unwrap().is_empty());
        let st = block_on(ob.stats()).unwrap();
        assert_eq!(st[0].pending, 2);
        assert_eq!(st[0].last_error.as_deref(), Some("сеть"));

        let due = block_on(ob.fetch_due(next, 10)).unwrap();
        assert_eq!(due[0].attempts, 1);
        block_on(ob.mark_done(due[0].id, next)).unwrap();
        block_on(ob.mark_done(due[1].id, next)).unwrap();
        assert!(block_on(ob.fetch_due(next, 10)).unwrap().is_empty());
        assert_eq!(block_on(ob.stats()).unwrap()[0].pending, 0);
        assert_eq!(ob.events().len(), 3);
    }
}
