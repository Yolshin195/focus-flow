//! Сборка сервиса Focus Flow: конфиг, data dir, токен, [`App`] (хранилище +
//! [`focus_core::FocusService`] + шина [`ServerEvent`]), фоновый таймер
//! (закрытие сессий по лимиту, системные уведомления) и запуск Axum на
//! `127.0.0.1:7878` с остановкой по Ctrl+C.
//!
//! [`ServerEvent`]: focus_api_types::ServerEvent

mod app;
mod config;
mod notify;
mod server;
mod timer;
mod token;

pub use app::App;
pub use config::{Config, DEFAULT_ADDR};
pub use notify::{LogNotifier, Notifier, default_notifier, notification_for};
pub use server::{router, run};
pub use timer::{POLL, run_timer};
pub use token::load_or_create_token;
