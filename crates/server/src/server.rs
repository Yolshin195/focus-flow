//! Запуск: Axum на адресе конфига, таймер, остановка по Ctrl+C / SIGTERM.

use anyhow::Context as _;
use axum::Router;
use axum::routing::get;
use tokio_util::sync::CancellationToken;

use crate::app::App;
use crate::config::Config;
use crate::notify::default_notifier;
use crate::timer::{POLL, run_timer};

/// HTTP-роутер сервиса.
pub fn router(_app: &App) -> Router {
    Router::new().route(
        "/api/health",
        get(|| async { axum::Json(serde_json::json!({"status": "ok"})) }),
    )
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

    let cancel = CancellationToken::new();
    let timer = tokio::spawn(run_timer(
        app.clone(),
        default_notifier(),
        POLL,
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
