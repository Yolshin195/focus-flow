//! Дерево задач: задача верхнего уровня → пункты.

use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};

use crate::ids::TaskId;

/// Статус задачи или пункта.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    /// Запланирована (по умолчанию).
    #[default]
    Planned,
    /// В работе.
    InProgress,
    /// Готова.
    Done,
    /// Отменена.
    Cancelled,
}

/// Узел дерева задач. Пункт — узел без детей; в работу берётся только он.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskDto {
    /// Id.
    pub id: TaskId,
    /// Родитель; `None` у задачи верхнего уровня и у пункта без родителя.
    #[serde(default)]
    pub parent_id: Option<TaskId>,
    /// Название.
    pub title: String,
    /// Заметки.
    #[serde(default)]
    pub notes: Option<String>,
    /// Статус.
    pub status: TaskStatus,
    /// На какой день запланирована.
    #[serde(default)]
    pub planned_for: Option<NaiveDate>,
    /// Оценка в интервалах по 50 минут.
    #[serde(default)]
    pub estimate_intervals: Option<u32>,
    /// Порядок среди соседей.
    #[serde(default)]
    pub sort_order: i64,
    /// Создана.
    pub created_at: DateTime<Utc>,
    /// Когда стала `done`.
    #[serde(default)]
    pub done_at: Option<DateTime<Utc>>,
    /// Ссылка на внешний источник (например, задачу Jira), если есть.
    #[serde(default)]
    pub external_url: Option<String>,
    /// Пункты (дети) в порядке `sort_order`.
    #[serde(default)]
    pub children: Vec<TaskDto>,
}

impl TaskDto {
    /// Пункт — узел без детей; только его можно взять в работу.
    pub fn is_item(&self) -> bool {
        self.children.is_empty()
    }

    /// Поиск узла по id в этом поддереве.
    pub fn find(&self, id: TaskId) -> Option<&TaskDto> {
        if self.id == id {
            return Some(self);
        }
        self.children.iter().find_map(|c| c.find(id))
    }
}

/// Создание задачи или пункта: тело `POST /api/tasks`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct NewTask {
    /// Родитель; `None` — узел верхнего уровня.
    #[serde(default)]
    pub parent_id: Option<TaskId>,
    /// Название (не пустое).
    pub title: String,
    /// Заметки.
    #[serde(default)]
    pub notes: Option<String>,
    /// На какой день запланирована.
    #[serde(default)]
    pub planned_for: Option<NaiveDate>,
    /// Оценка в интервалах по 50 минут.
    #[serde(default)]
    pub estimate_intervals: Option<u32>,
}

impl NewTask {
    /// Узел верхнего уровня с названием.
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            ..Self::default()
        }
    }

    /// Пункт внутри задачи `parent_id`.
    pub fn child(parent_id: TaskId, title: impl Into<String>) -> Self {
        Self {
            parent_id: Some(parent_id),
            ..Self::new(title)
        }
    }
}

/// Частичная правка: тело `PATCH /api/tasks/{id}`.
///
/// Отсутствующее поле — «не менять». Для полей, которые можно очистить,
/// `Some(None)` (в JSON — `null`) — «очистить».
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskPatch {
    /// Новое название.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Новые заметки; `Some(None)` — очистить.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::double_option"
    )]
    pub notes: Option<Option<String>>,
    /// Новый статус.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<TaskStatus>,
    /// Новый день; `Some(None)` — снять с дня.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::double_option"
    )]
    pub planned_for: Option<Option<NaiveDate>>,
    /// Новая оценка; `Some(None)` — убрать оценку.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::double_option"
    )]
    pub estimate_intervals: Option<Option<u32>>,
    /// Новый порядок среди соседей.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sort_order: Option<i64>,
}

/// Импорт задачи из внешнего источника: тело `POST /api/import`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImportRequest {
    /// Ссылка или ключ (например, `PROJ-123`).
    pub url: String,
}

/// Результат импорта.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImportResult {
    /// Id плагина-источника (например, `jira`).
    pub source: String,
    /// `true` — задача создана; `false` — уже была, догружены новые пункты.
    pub created: bool,
    /// Сколько пунктов добавлено этим импортом.
    pub added_items: u32,
    /// Задача с пунктами после импорта.
    pub task: TaskDto,
}
