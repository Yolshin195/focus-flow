//! Доменные события — то, на что реагируют плагины (через outbox).
//!
//! По SPEC `DomainEvent` относится к `plugin-api`; он определён здесь, потому
//! что его публикует ядро, а `plugin-api` реэкспортирует его (см. DECISIONS).

use serde::{Deserialize, Serialize};

use crate::domain::{Session, Task};

/// Доменное событие. В outbox хранится как JSON `{"type": ..., "data": ...}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum DomainEvent {
    /// Пользователь создал узел (задачу или пункт). При импорте не публикуется.
    ItemCreated {
        /// Созданный узел.
        task: Task,
        /// Родитель, если есть (плагину нужна его внешняя ссылка).
        parent: Option<Task>,
    },
    /// Узел стал `done` («Готово» или правка статуса).
    ItemCompleted {
        /// Узел после изменения.
        task: Task,
        /// Родитель, если есть.
        parent: Option<Task>,
    },
    /// Готовый узел снова открыт (`done` → `planned`/`in_progress`): сняли
    /// отметку. Переход `done` → `cancelled` его не публикует.
    ItemReopened {
        /// Узел после изменения.
        task: Task,
        /// Родитель, если есть.
        parent: Option<Task>,
    },
    /// Сессия закрыта (любой вид и причина).
    SessionClosed {
        /// Закрытая сессия.
        session: Session,
        /// Пункт сессии `work`.
        item: Option<Task>,
        /// Родитель пункта, если есть.
        parent: Option<Task>,
    },
    /// Задача импортирована или догружена из внешнего источника.
    TaskImported {
        /// Id плагина-источника.
        source: String,
        /// Задача верхнего уровня.
        task: Task,
        /// `true` — создана; `false` — уже была.
        created: bool,
    },
    /// Событие из более новой версии (в старой записи outbox).
    #[serde(other)]
    Unknown,
}

impl DomainEvent {
    /// Имя типа события.
    pub fn name(&self) -> &'static str {
        match self {
            Self::ItemCreated { .. } => "item_created",
            Self::ItemCompleted { .. } => "item_completed",
            Self::ItemReopened { .. } => "item_reopened",
            Self::SessionClosed { .. } => "session_closed",
            Self::TaskImported { .. } => "task_imported",
            Self::Unknown => "unknown",
        }
    }
}
