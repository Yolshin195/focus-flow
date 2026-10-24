//! Сессии: виды, причины завершения, активная сессия и запись о сессии.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::ids::{DayId, SessionId, TaskId};

/// Вид сессии.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionKind {
    /// Работа над пунктом: лимит (по умолчанию 50 мин), отсчёт вниз.
    Work,
    /// Отдых: фиксированная длина (по умолчанию 10 мин), пропустить нельзя.
    Rest,
    /// Планирование: без лимита, отсчёт вверх, идёт в норму.
    Planning,
}

/// Причина завершения сессии.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EndReason {
    /// «Готово»: пункт выполнен.
    TaskDone,
    /// Истёк лимит сессии (для отдыха — штатное окончание).
    Limit,
    /// «Прервать»: работа закрыта досрочно без завершения пункта.
    Interrupted,
    /// «Стоп» планирования.
    Stopped,
    /// Сессия закрыта стартом другой (например, планирование → работа).
    Switched,
}

/// Что делать после «Готово».
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DoneNext {
    /// Сразу начать отдых.
    Rest,
    /// Сразу начать планирование (без потерянного времени).
    Planning,
}

/// Краткая ссылка на пункт, над которым идёт работа.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ItemRef {
    /// Id пункта.
    pub id: TaskId,
    /// Название пункта.
    pub title: String,
    /// Родительская задача, если есть.
    #[serde(default)]
    pub parent_id: Option<TaskId>,
    /// Название родительской задачи, если есть.
    #[serde(default)]
    pub parent_title: Option<String>,
}

/// Активная сессия. Каждый вариант несёт ровно те поля, которые для него
/// имеют смысл: у работы всегда есть пункт и `ends_at`, у планирования
/// `ends_at` нет. В JSON — объект с тегом `kind`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ActiveSession {
    /// Идёт работа над пунктом.
    Work {
        /// Id сессии.
        session_id: SessionId,
        /// Пункт в работе.
        item: ItemRef,
        /// Начало сессии.
        started_at: DateTime<Utc>,
        /// Плановый конец: `started_at + work_min`.
        ends_at: DateTime<Utc>,
    },
    /// Идёт отдых.
    Rest {
        /// Id сессии.
        session_id: SessionId,
        /// Начало отдыха.
        started_at: DateTime<Utc>,
        /// Конец отдыха: `started_at + rest_min`.
        ends_at: DateTime<Utc>,
    },
    /// Идёт планирование (без лимита).
    Planning {
        /// Id сессии.
        session_id: SessionId,
        /// Начало планирования.
        started_at: DateTime<Utc>,
    },
}

impl ActiveSession {
    /// Вид сессии.
    pub fn kind(&self) -> SessionKind {
        match self {
            Self::Work { .. } => SessionKind::Work,
            Self::Rest { .. } => SessionKind::Rest,
            Self::Planning { .. } => SessionKind::Planning,
        }
    }

    /// Id сессии.
    pub fn session_id(&self) -> SessionId {
        match self {
            Self::Work { session_id, .. }
            | Self::Rest { session_id, .. }
            | Self::Planning { session_id, .. } => *session_id,
        }
    }

    /// Начало сессии.
    pub fn started_at(&self) -> DateTime<Utc> {
        match self {
            Self::Work { started_at, .. }
            | Self::Rest { started_at, .. }
            | Self::Planning { started_at, .. } => *started_at,
        }
    }

    /// Плановый конец; у планирования его нет.
    pub fn ends_at(&self) -> Option<DateTime<Utc>> {
        match self {
            Self::Work { ends_at, .. } | Self::Rest { ends_at, .. } => Some(*ends_at),
            Self::Planning { .. } => None,
        }
    }
}

/// Запись о сессии (активной или завершённой) — для календаря и отчётов.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionDto {
    /// Id сессии.
    pub id: SessionId,
    /// День, к которому относится сессия (день старта).
    pub day_id: DayId,
    /// Вид сессии.
    pub kind: SessionKind,
    /// Пункт (только у `work`).
    #[serde(default)]
    pub task_id: Option<TaskId>,
    /// Название пункта на момент чтения (для подписи в календаре).
    #[serde(default)]
    pub task_title: Option<String>,
    /// Начало.
    pub started_at: DateTime<Utc>,
    /// Конец; `None`, пока сессия активна.
    #[serde(default)]
    pub ended_at: Option<DateTime<Utc>>,
    /// Лимит в секундах; `None` у планирования.
    #[serde(default)]
    pub limit_sec: Option<i64>,
    /// Причина завершения; `None`, пока сессия активна.
    #[serde(default)]
    pub end_reason: Option<EndReason>,
    /// Фактическая длительность в секундах (у активной — на момент `server_time`).
    pub duration_sec: i64,
}

/// Ответ на завершение активной сессии («Готово», «Прервать», «Стоп»):
/// закрытая сессия и состояние после действия.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionEndResult {
    /// Только что закрытая сессия (с `ended_at` и `end_reason`).
    pub ended: SessionDto,
    /// Статус после действия (например, уже с активным отдыхом).
    pub status: crate::StatusDto,
}
