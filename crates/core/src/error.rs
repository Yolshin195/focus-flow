//! Ошибки ядра. Варианты совпадают с бизнес-ошибками контракта
//! ([`ApiError`]), плюс [`CoreError::Storage`] — сбой порта.

use focus_api_types::{ApiError, SessionKind};

/// Результат операции ядра.
pub type CoreResult<T> = Result<T, CoreError>;

/// Ошибка ядра.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CoreError {
    /// Нет активной сессии.
    #[error("нет активной сессии")]
    NoActiveSession,
    /// Уже идёт сессия, которую нельзя прервать этой командой.
    #[error("уже идёт активная сессия")]
    SessionAlreadyActive,
    /// Команда применима к другому виду сессии.
    #[error("команда не подходит к текущей сессии: ожидалась {expected:?}, идёт {actual:?}")]
    WrongSessionKind {
        /// Нужный вид.
        expected: SessionKind,
        /// Текущий вид.
        actual: SessionKind,
    },
    /// В работу берётся пункт, а не задача с пунктами.
    #[error("в работу берётся пункт, а не задача с пунктами")]
    ItemRequired,
    /// Узел не найден.
    #[error("задача или пункт не найдены")]
    ItemNotFound,
    /// Сегодняшний день закрыт.
    #[error("день закрыт")]
    DayClosed,
    /// Нет подходящего или работающего плагина.
    #[error("плагин недоступен: {0}")]
    PluginUnavailable(String),
    /// Неверные входные данные.
    #[error("ошибка в данных: {0}")]
    Validation(String),
    /// Сбой хранилища или другого порта.
    #[error("ошибка хранилища: {0}")]
    Storage(String),
}

impl CoreError {
    /// Сбой порта с текстом.
    pub fn storage(e: impl std::fmt::Display) -> Self {
        Self::Storage(e.to_string())
    }
}

impl From<CoreError> for ApiError {
    fn from(e: CoreError) -> Self {
        match e {
            CoreError::NoActiveSession => Self::NoActiveSession,
            CoreError::SessionAlreadyActive => Self::SessionAlreadyActive,
            CoreError::WrongSessionKind { expected, actual } => {
                Self::WrongSessionKind { expected, actual }
            }
            CoreError::ItemRequired => Self::ItemRequired,
            CoreError::ItemNotFound => Self::ItemNotFound,
            CoreError::DayClosed => Self::DayClosed,
            CoreError::PluginUnavailable(s) => Self::PluginUnavailable(s),
            CoreError::Validation(s) => Self::Validation(s),
            CoreError::Storage(s) => Self::Internal(s),
        }
    }
}
