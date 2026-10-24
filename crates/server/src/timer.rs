//! Фоновый таймер: спит до `next_deadline()` (но не дольше [`POLL`]),
//! вызывает `tick(now)` и рассылает события клиентам. Уведомлений сервис не
//! показывает: звать человека — дело клиента (сервис может работать на VDS).
//!
//! `tick` вызывается и при каждом опросе: он же отдаёт события сессий,
//! закрытых «догоном» внутри команд, а сам по себе дешёвый.

use std::time::Duration;

use tokio_util::sync::CancellationToken;

use crate::app::App;

/// Наибольший сон таймера: страховка от сдвига часов, сна системы и
/// команд, поменявших дедлайн.
pub const POLL: Duration = Duration::from_secs(1);

/// Цикл таймера до отмены `cancel`. Ошибки шага логируются, цикл продолжается.
pub async fn run_timer(app: App, poll: Duration, cancel: CancellationToken) {
    tracing::debug!("таймер запущен");
    loop {
        if let Err(e) = step(&app).await {
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

async fn step(app: &App) -> anyhow::Result<()> {
    let now = app.service.ports().clock.now();
    for ev in app.service.tick(now).await? {
        tracing::debug!(event = ev.name(), "событие таймера");
        app.broadcast(ev);
    }
    Ok(())
}
