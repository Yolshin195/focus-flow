//! Порты ядра — трейты, через которые оно видит внешний мир. Реализации:
//! [`crate::memory`] (тесты, мок) и `focus-storage` (SQLite), источники задач —
//! плагины в `focus-server`.
//!
//! Все методы асинхронные и возвращают `Send`-future (`#[async_trait]`),
//! чтобы сервис можно было вызывать из обработчиков Axum.

use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, NaiveDate, Utc};
use focus_api_types::{DayId, SessionId, TaskId};

use crate::clock::Clock;
use crate::domain::{ImportedTask, Session, Settings, Task, WorkDay};
use crate::error::CoreResult;
use crate::events::DomainEvent;
use crate::outbox::{OutboxEntry, OutboxId, OutboxStats};

/// Рабочие дни.
#[async_trait]
pub trait DayRepo: Send + Sync {
    /// День по id.
    async fn get(&self, id: DayId) -> CoreResult<Option<WorkDay>>;
    /// День по локальной дате (не больше одного на дату).
    async fn by_date(&self, date: NaiveDate) -> CoreResult<Option<WorkDay>>;
    /// Вставить или заменить день.
    async fn save(&self, day: &WorkDay) -> CoreResult<()>;
}

/// Дерево задач.
#[async_trait]
pub trait TaskRepo: Send + Sync {
    /// Узел по id.
    async fn get(&self, id: TaskId) -> CoreResult<Option<Task>>;
    /// Все узлы (порядок не важен — дерево строит сервис).
    async fn list(&self) -> CoreResult<Vec<Task>>;
    /// Дети узла `parent`; при `None` — узлы верхнего уровня.
    async fn children(&self, parent: Option<TaskId>) -> CoreResult<Vec<Task>>;
    /// Узел с данной внешней ссылкой (для повторного импорта).
    async fn by_external_url(&self, url: &str) -> CoreResult<Option<Task>>;
    /// Вставить или заменить узел.
    async fn save(&self, task: &Task) -> CoreResult<()>;
}

/// Сессии.
#[async_trait]
pub trait SessionRepo: Send + Sync {
    /// Сессия по id.
    async fn get(&self, id: SessionId) -> CoreResult<Option<Session>>;
    /// Активная сессия (`ended_at IS NULL`); их не больше одной.
    async fn active(&self) -> CoreResult<Option<Session>>;
    /// Сессии дня в порядке `started_at`.
    async fn by_day(&self, day_id: DayId) -> CoreResult<Vec<Session>>;
    /// Вставить или заменить сессию.
    async fn save(&self, session: &Session) -> CoreResult<()>;
}

/// Настройки.
#[async_trait]
pub trait SettingsRepo: Send + Sync {
    /// Текущие настройки (по умолчанию, если ничего не сохранено).
    async fn load(&self) -> CoreResult<Settings>;
    /// Сохранить настройки целиком.
    async fn save(&self, settings: &Settings) -> CoreResult<()>;
}

/// Шина доменных событий: в сервисе — запись в таблицу `outbox`, откуда
/// воркер доставляет события плагинам с повторами.
#[async_trait]
pub trait EventSink: Send + Sync {
    /// Поставить события в очередь (порядок сохраняется).
    async fn publish(&self, events: &[DomainEvent]) -> CoreResult<()>;
}

/// Очередь outbox для воркера доставки (шаг 5). Публикация
/// ([`EventSink::publish`]) раскладывает событие на записи — по одной на
/// каждого подписчика (`plugin_id`); без подписчиков событие никуда не пишется.
#[async_trait]
pub trait Outbox: EventSink {
    /// Задать подписчиков (id включённых плагинов) для новых публикаций.
    async fn set_subscribers(&self, plugin_ids: &[String]) -> CoreResult<()>;
    /// Записи, готовые к доставке в момент `now`, в порядке публикации; по
    /// каждому плагину порядок сохраняется (см. [`crate::outbox::select_due`]).
    async fn fetch_due(&self, now: DateTime<Utc>, limit: usize) -> CoreResult<Vec<OutboxEntry>>;
    /// Запись доставлена.
    async fn mark_done(&self, id: OutboxId, now: DateTime<Utc>) -> CoreResult<()>;
    /// Доставка не удалась: `attempts + 1`, ошибка и следующая попытка через
    /// [`crate::outbox::backoff`]. Возвращает момент следующей попытки.
    async fn mark_failed(
        &self,
        id: OutboxId,
        error: &str,
        now: DateTime<Utc>,
    ) -> CoreResult<DateTime<Utc>>;
    /// Сводка очереди по плагинам (только плагины, у которых есть записи).
    async fn stats(&self) -> CoreResult<Vec<OutboxStats>>;
}

/// Реестр источников задач (плагины `TaskSource`).
#[async_trait]
pub trait TaskSources: Send + Sync {
    /// Получить задачу по ссылке/ключу через подходящий источник.
    /// Нет подходящего — [`crate::CoreError::PluginUnavailable`].
    async fn import(&self, url: &str) -> CoreResult<ImportedTask>;
}

/// Реестр без источников: любой импорт → `PluginUnavailable`.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoTaskSources;

#[async_trait]
impl TaskSources for NoTaskSources {
    async fn import(&self, url: &str) -> CoreResult<ImportedTask> {
        Err(crate::CoreError::PluginUnavailable(format!(
            "нет источника задач для «{url}»"
        )))
    }
}

/// Набор портов, из которого собирается [`crate::FocusService`].
#[derive(Clone)]
pub struct Ports {
    /// Часы.
    pub clock: Arc<dyn Clock>,
    /// Дни.
    pub days: Arc<dyn DayRepo>,
    /// Задачи.
    pub tasks: Arc<dyn TaskRepo>,
    /// Сессии.
    pub sessions: Arc<dyn SessionRepo>,
    /// Настройки.
    pub settings: Arc<dyn SettingsRepo>,
    /// Шина доменных событий (outbox).
    pub events: Arc<dyn EventSink>,
    /// Источники задач для импорта.
    pub sources: Arc<dyn TaskSources>,
}
