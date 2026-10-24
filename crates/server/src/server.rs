//! Запуск: Axum на адресе конфига, таймер, воркер outbox, остановка по
//! Ctrl+C / SIGTERM.

use anyhow::Context as _;
use std::sync::Arc;

use axum::Router;
use focus_api_rest::{Access, RestState};

use crate::app::App;
use crate::config::Config;
use crate::notify::default_notifier;
use crate::plugins::{OUTBOX_POLL, run_outbox_worker};
use crate::timer::{POLL, run_timer};

/// HTTP-роутер сервиса: REST + SSE (`/api/*`), маршруты плагинов
/// (`/api/plugins/<id>/…`) и MCP (`/mcp`, фича `mcp`) — за одними
/// проверками `Host`/`Origin`/токена.
pub fn router(app: &App) -> Router {
    let access = Arc::new(Access {
        token: app.token.to_string(),
        allowed_hosts: app.config.allowed_hosts.clone(),
        allowed_origins: app.config.allowed_origins.clone(),
    });
    let rest = focus_api_rest::router(RestState {
        service: app.service.clone(),
        events: app.events.clone(),
        access: access.clone(),
        shutdown: app.shutdown.clone(),
    });
    let extra = app.plugins.routes();
    #[cfg(feature = "mcp")]
    let extra = extra.merge(focus_api_mcp::router(app.mcp.clone()));
    let extra = extra.layer(axum::middleware::from_fn_with_state(
        access,
        focus_api_rest::guard::guard,
    ));
    Router::new().merge(rest).merge(extra)
}

/// Запустить сервис и работать до Ctrl+C (или SIGTERM): открыть БД и токен,
/// поднять таймер и HTTP. При остановке — дождаться запросов и таймера,
/// закрыть БД.
pub async fn run(config: Config) -> anyhow::Result<()> {
    let addr = config.addr;
    if !addr.ip().is_loopback() {
        tracing::warn!(%addr, "сервис слушает не только loopback — доступ защищает только токен");
    }
    let app = App::open(config).await?;
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .with_context(|| format!("не удалось занять {addr} — сервис уже запущен?"))?;
    tracing::info!(addr = %listener.local_addr()?, data_dir = %app.config.data_dir.display(), "focus-flow serve запущен");

    let cancel = app.shutdown.clone();
    let timer = tokio::spawn(run_timer(
        app.clone(),
        default_notifier(),
        POLL,
        cancel.clone(),
    ));

    let outbox = tokio::spawn(run_outbox_worker(
        app.plugins.clone(),
        OUTBOX_POLL,
        cancel.clone(),
    ));

    let shutdown = {
        let cancel = cancel.clone();
        async move {
            shutdown_signal().await;
            tracing::info!("остановка…");
            cancel.cancel();
        }
    };
    let served = axum::serve(listener, router(&app))
        .with_graceful_shutdown(shutdown)
        .await
        .context("ошибка HTTP-сервера");

    cancel.cancel();
    if let Err(e) = timer.await {
        tracing::error!(error = %e, "таймер завершился с ошибкой");
    }
    if let Err(e) = outbox.await {
        tracing::error!(error = %e, "воркер outbox завершился с ошибкой");
    }
    app.storage.close().await;
    tracing::info!("сервис остановлен");
    served
}

async fn shutdown_signal() {
    let ctrl_c = async {
        if let Err(e) = tokio::signal::ctrl_c().await {
            tracing::error!(error = %e, "не удалось подписаться на Ctrl+C");
            std::future::pending::<()>().await;
        }
    };
    #[cfg(unix)]
    let term = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut s) => {
                s.recv().await;
            }
            Err(e) => {
                tracing::error!(error = %e, "не удалось подписаться на SIGTERM");
                std::future::pending::<()>().await;
            }
        }
    };
    #[cfg(not(unix))]
    let term = std::future::pending::<()>();
    tokio::select! {
        () = ctrl_c => {}
        () = term => {}
    }
}
