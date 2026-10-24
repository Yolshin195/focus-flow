//! Сборка сервиса Focus Flow: конфиг, data dir, токен, [`App`] (хранилище +
//! [`focus_core::FocusService`] + шина [`ServerEvent`]), фоновый таймер
//! (закрытие сессий по лимиту; уведомлений сервис не показывает) и запуск Axum на
//! `127.0.0.1:7878` с остановкой по Ctrl+C; реестр плагинов и воркер
//! доставки outbox ([`PluginRegistry`], [`run_outbox_worker`]).
//!
//! [`ServerEvent`]: focus_api_types::ServerEvent

mod app;
mod config;
mod plugins;
mod server;
mod timer;
mod token;

pub use app::App;
pub use config::{Config, DEFAULT_ADDR};
pub use plugins::{
    OUTBOX_POLL, PluginRegistry, Registration, builtin_plugins, plugin_enabled, plugin_enabled_or,
    run_outbox_worker,
};
pub use server::{router, run, run_until};
pub use timer::{POLL, run_timer};
pub use token::load_or_create_token;
