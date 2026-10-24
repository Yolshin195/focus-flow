//! События сервиса для SSE `GET /api/events`.

use serde::{Deserialize, Serialize};

use crate::day::StatusDto;
use crate::ids::TaskId;
use crate::session::SessionDto;

/// Событие сервиса.
///
/// В SSE: поле `event:` — [`ServerEvent::name`], поле `data:` — JSON всего
/// события вида `{"type": "...", "data": ...}`. Неизвестный клиенту `type`
/// читается как [`ServerEvent::Unknown`] — его нужно просто пропустить.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum ServerEvent {
    /// Изменилось состояние (старт/конец сессии, открытие/закрытие дня, настройки).
    StatusChanged(StatusDto),
    /// Сессия закончилась (в том числе по лимиту без участия клиента).
    SessionEnded {
        /// Закрытая сессия.
        session: SessionDto,
    },
    /// Задача или пункт изменились (создание, правка, импорт, «Готово»).
    TaskChanged {
        /// Id изменённого узла.
        task_id: TaskId,
    },
    /// Плагин не смог обработать событие (доставка будет повторена).
    PluginError {
        /// Id плагина.
        plugin: String,
        /// Текст ошибки.
        message: String,
    },
    /// Событие из более новой версии сервиса.
    #[serde(other)]
    Unknown,
}

impl ServerEvent {
    /// Имя события для поля `event:` в SSE.
    pub fn name(&self) -> &'static str {
        match self {
            Self::StatusChanged(_) => "status_changed",
            Self::SessionEnded { .. } => "session_ended",
            Self::TaskChanged { .. } => "task_changed",
            Self::PluginError { .. } => "plugin_error",
            Self::Unknown => "unknown",
        }
    }
}
