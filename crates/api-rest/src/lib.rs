//! REST API и SSE (`/api/events`) сервиса Focus Flow на Axum.
//! Все ответы — DTO из `focus-api-types`, ошибки — [`ApiError`] в JSON
//! (`{"code", "details"}`) с HTTP-статусом [`ApiError::http_status`].
//!
//! Защита ([`guard`]): проверка `Host` и `Origin` (DNS rebinding, чужие
//! страницы), CORS только для разрешённых `Origin`, токен
//! `Authorization: Bearer` (для SSE ещё `?token=`). `/api/health` — без токена.
//!
//! [`ApiError`]: focus_api_types::ApiError
//! [`ApiError::http_status`]: focus_api_types::ApiError::http_status

mod error;
pub mod guard;
mod handlers;
mod sse;

use std::sync::Arc;

use axum::Router;
use axum::routing::{get, patch, post};
use focus_api_types::ServerEvent;
use focus_core::FocusService;
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;

pub use error::{ApiJson, ApiPath, RestError};
pub use guard::Access;

/// Состояние обработчиков REST.
#[derive(Clone)]
pub struct RestState {
    /// Прикладной сервис ядра.
    pub service: Arc<FocusService>,
    /// Шина событий: команды публикуют сюда, SSE читает.
    pub events: broadcast::Sender<ServerEvent>,
    /// Токен и разрешённые `Host`/`Origin`.
    pub access: Arc<Access>,
    /// Отмена при остановке сервиса: закрывает потоки SSE, иначе graceful
    /// shutdown ждал бы их вечно.
    pub shutdown: CancellationToken,
}

impl RestState {
    pub(crate) fn broadcast(&self, event: ServerEvent) {
        // Ошибка — только «нет подписчиков».
        let _ = self.events.send(event);
    }
}

/// Роутер REST + SSE со всеми проверками доступа. Пути — по таблице
/// «REST API» в SPEC.
pub fn router(state: RestState) -> Router {
    let api = Router::new()
        .route("/api/health", get(handlers::health))
        .route("/api/status", get(handlers::status))
        .route("/api/day/open", post(handlers::open_day))
        .route("/api/day/close", post(handlers::close_day))
        .route("/api/days/{date}", get(handlers::day))
        .route(
            "/api/tasks",
            get(handlers::list_tasks).post(handlers::create_task),
        )
        .route("/api/tasks/{id}", patch(handlers::update_task))
        .route("/api/sessions/work", post(handlers::start_work))
        .route("/api/sessions/planning", post(handlers::start_planning))
        .route("/api/sessions/rest", post(handlers::start_rest))
        .route("/api/sessions/active/done", post(handlers::done))
        .route("/api/sessions/active/interrupt", post(handlers::interrupt))
        .route("/api/sessions/active/stop", post(handlers::stop))
        .route("/api/import", post(handlers::import))
        .route(
            "/api/settings",
            get(handlers::get_settings).put(handlers::put_settings),
        )
        .route("/api/plugins/status", get(handlers::plugin_status))
        .route("/api/events", get(sse::events));
    api.layer(axum::middleware::from_fn_with_state(
        state.access.clone(),
        guard::guard,
    ))
    .with_state(state)
}
