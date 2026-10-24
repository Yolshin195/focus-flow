//! Ядро Focus Flow: домен (день, сессии, задачи), машина состояний сессий
//! и все правила таймера 50/10. Не знает про SQL, HTTP, Axum и плагины —
//! внешний мир подключается через трейты-порты (`Clock`, репозитории, шина событий).
//!
//! - [`FocusService`] — всё поведение за контрактом `FocusApi`: inherent-методы
//!   с `Send`-future (для сервера) и реализация [`focus_api_types::FocusApi`];
//!   [`FocusService::tick`] — шаг фонового таймера.
//! - [`ports`] — трейты-порты, [`memory`] — их реализации в памяти.
//! - [`rules`] — чистые правила (потерянное время, итоги, дерево задач).

pub mod clock;
pub mod domain;
pub mod error;
pub mod events;
pub mod memory;
pub mod outbox;
pub mod ports;
pub mod rules;
mod service;

pub use clock::{Clock, ManualClock, SystemClock};
pub use domain::{ImportedItem, ImportedTask, Session, Settings, Task, WorkDay};
pub use error::{CoreError, CoreResult};
pub use events::DomainEvent;
pub use outbox::{OutboxEntry, OutboxId, OutboxStats};
pub use ports::{
    DayRepo, EventSink, NoPlugins, NoTaskSources, Outbox, PluginHost, Ports, SessionRepo,
    SettingsRepo, TaskRepo, TaskSources,
};
pub use service::{FocusService, session_dto};
