//! Фоновый таймер: спит до `next_deadline()` (но не дольше [`POLL`]),
//! вызывает `tick(now)`, рассылает события и показывает уведомления.
//!
//! `tick` вызывается и при каждом опросе: он же отдаёт события сессий,
//! закрытых «догоном» внутри команд, а сам по себе дешёвый.

use std::sync::Arc;
use std::time::Duration;

use tokio_util::sync::CancellationToken;

use crate::app::App;
use crate::notify::{Notifier, notification_for};

/// Наибольший сон таймера: страховка от сдвига часов, сна системы и
/// команд, поменявших дедлайн.
pub const POLL: Duration = Duration::from_secs(1);

/// Цикл таймера до отмены `cancel`. Ошибки шага логируются, цикл продолжается.
pub async fn run_timer(
    app: App,
    notifier: Arc<dyn Notifier>,
    poll: Duration,
    cancel: CancellationToken,
) {
    tracing::debug!("таймер запущен");
    loop {
        if let Err(e) = step(&app, &notifier).await {
            tracing::error!(error = %e, "ошибка шага таймера");
        }
        let sleep = match app.service.next_deadline().await {
            Ok(Some(deadline)) => {
                let now = app.service.ports().clock.now();
                (deadline - now)
                    .to_std()
                    .unwrap_or(Duration::ZERO)
                    .min(poll)
            }
            Ok(None) => poll,
            Err(e) => {
                tracing::error!(error = %e, "таймер: не удалось прочитать дедлайн");
                poll
            }
        };
        tokio::select! {
            () = cancel.cancelled() => break,
            () = tokio::time::sleep(sleep) => {}
        }
    }
    tracing::debug!("таймер остановлен");
}

async fn step(app: &App, notifier: &Arc<dyn Notifier>) -> anyhow::Result<()> {
    let now = app.service.ports().clock.now();
    let events = app.service.tick(now).await?;
    if events.is_empty() {
        return Ok(());
    }
    let rest_min = app.service.settings().await?.rest_min;
    for ev in events {
        if let Some((title, body)) = notification_for(&ev, rest_min) {
            let n = notifier.clone();
            // Показ может блокировать (D-Bus) — не держим runtime.
            tokio::task::spawn_blocking(move || n.notify(&title, &body));
        }
        tracing::debug!(event = ev.name(), "событие таймера");
        app.broadcast(ev);
    }
    Ok(())
}
