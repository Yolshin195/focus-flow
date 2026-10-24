//! Ошибки API. Одинаковы для REST, MCP и клиентских реализаций `FocusApi`.

use serde::{Deserialize, Serialize};

use crate::session::SessionKind;

/// Ошибка операции API.
///
/// В JSON — `{"code": "...", "details": ...}`; `details` есть только у вариантов
/// с данными. Неизвестный клиенту `code` читается как [`ApiError::Unknown`].
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error, Serialize, Deserialize)]
#[serde(tag = "code", content = "details", rename_all = "snake_case")]
pub enum ApiError {
    /// Команда требует активной сессии, а её нет.
    #[error("нет активной сессии")]
    NoActiveSession,
    /// Уже идёт сессия, которую нельзя прервать этой командой
    /// (например, отдых или работа).
    #[error("уже идёт активная сессия")]
    SessionAlreadyActive,
    /// Команда применима к другому виду активной сессии
    /// (например, «Стоп» во время работы).
    #[error("команда не подходит к текущей сессии: ожидалась {expected:?}, идёт {actual:?}")]
    WrongSessionKind {
        /// Какой вид сессии нужен команде.
        expected: SessionKind,
        /// Какой вид идёт сейчас.
        actual: SessionKind,
    },
    /// В работу берётся пункт (узел без детей), а передана задача с пунктами.
    #[error("в работу берётся пункт, а не задача с пунктами")]
    ItemRequired,
    /// Задача или пункт с таким id не найдены.
    #[error("задача или пункт не найдены")]
    ItemNotFound,
    /// Сегодняшний день закрыт: новые сессии не стартуют, пока день не открыт снова.
    #[error("день закрыт")]
    DayClosed,
    /// Плагин выключен, не настроен или недоступен; в строке — подробности.
    #[error("плагин недоступен: {0}")]
    PluginUnavailable(String),
    /// Неверные входные данные; в строке — что именно.
    #[error("ошибка в данных: {0}")]
    Validation(String),
    /// Нет или неверный токен доступа.
    #[error("нет доступа: неверный токен")]
    Unauthorized,
    /// Запрос отклонён проверкой `Host`/`Origin` (защита от DNS rebinding и
    /// чужих страниц в браузере); в строке — что именно не так.
    #[error("запрос отклонён: {0}")]
    Forbidden(String),
    /// Клиент не смог связаться с сервисом (сеть, таймаут, неразборчивый ответ).
    /// Сервис эту ошибку не возвращает — её создаёт клиентская реализация `FocusApi`.
    #[error("нет связи с сервисом: {0}")]
    Transport(String),
    /// Внутренняя ошибка сервиса.
    #[error("внутренняя ошибка: {0}")]
    Internal(String),
    /// Код ошибки из более новой версии сервиса, неизвестный этому клиенту.
    #[serde(other)]
    #[error("неизвестная ошибка")]
    Unknown,
}

impl ApiError {
    /// HTTP-статус, с которым сервис отдаёт эту ошибку.
    pub fn http_status(&self) -> u16 {
        match self {
            Self::NoActiveSession
            | Self::SessionAlreadyActive
            | Self::WrongSessionKind { .. }
            | Self::DayClosed => 409,
            Self::ItemRequired | Self::Validation(_) => 422,
            Self::ItemNotFound => 404,
            Self::Unauthorized => 401,
            Self::Forbidden(_) => 403,
            Self::PluginUnavailable(_) => 503,
            Self::Transport(_) => 502,
            Self::Internal(_) | Self::Unknown => 500,
        }
    }

    /// Машинный код ошибки — то же значение, что поле `code` в JSON.
    pub fn code(&self) -> &'static str {
        match self {
            Self::NoActiveSession => "no_active_session",
            Self::SessionAlreadyActive => "session_already_active",
            Self::WrongSessionKind { .. } => "wrong_session_kind",
            Self::ItemRequired => "item_required",
            Self::ItemNotFound => "item_not_found",
            Self::DayClosed => "day_closed",
            Self::PluginUnavailable(_) => "plugin_unavailable",
            Self::Validation(_) => "validation",
            Self::Unauthorized => "unauthorized",
            Self::Forbidden(_) => "forbidden",
            Self::Transport(_) => "transport",
            Self::Internal(_) => "internal",
            Self::Unknown => "unknown",
        }
    }
}
