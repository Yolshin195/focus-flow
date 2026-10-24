//! Контракт между клиентами (Dioxus UI, тесты) и сервисом Focus Flow.
//!
//! Здесь только типы без логики: DTO, [`ActiveSession`], [`ApiError`],
//! события SSE [`ServerEvent`] и трейт [`FocusApi`]. Крейт собирается под
//! `wasm32-unknown-unknown`, поэтому зависит только от `serde`, `serde_json`,
//! `chrono`, `uuid`, `async-trait` и `thiserror`.
//!
//! Правила совместимости:
//! - новые поля DTO добавляются только опциональными и с `#[serde(default)]`;
//! - у [`ServerEvent`] и [`ApiError`] есть вариант `Unknown` (`#[serde(other)]`),
//!   поэтому старый клиент не падает на новом событии или коде ошибки;
//! - время — RFC3339 UTC (`DateTime<Utc>`), длительности — секунды (`*_sec`),
//!   настройки длин — минуты (`*_min`).

mod api;
mod day;
mod double_option;
mod error;
mod event;
mod ids;
mod session;
mod settings;
mod task;

pub use api::{ApiResult, FocusApi};
pub use day::{DayDto, DayInfoDto, DayTotalsDto, StatusDto, TimeRange};
pub use error::ApiError;
pub use event::ServerEvent;
pub use ids::{DayId, SessionId, TaskId};
pub use session::{
    ActiveSession, DoneNext, EndReason, ItemRef, SessionDto, SessionEndResult, SessionKind,
};
pub use settings::{PluginStatusDto, SettingsDto};
pub use task::{ImportRequest, ImportResult, NewTask, TaskDto, TaskPatch, TaskStatus};

/// Реэкспорт, чтобы клиентам не нужно было подключать `uuid` и `chrono` отдельно.
pub use chrono;
pub use uuid::{self, Uuid};

/// Тела запросов REST, у которых нет отдельного DTO.
pub mod requests {
    use serde::{Deserialize, Serialize};

    use crate::ids::TaskId;
    use crate::session::DoneNext;

    /// Тело `POST /api/sessions/work`.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
    pub struct StartWorkRequest {
        /// Пункт, над которым начинается работа.
        pub item_id: TaskId,
    }

    /// Тело `POST /api/sessions/active/done`.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
    pub struct DoneRequest {
        /// Что начать после «Готово».
        pub next: DoneNext,
    }
}

#[cfg(test)]
mod tests;
