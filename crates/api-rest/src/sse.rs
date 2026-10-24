//! SSE `GET /api/events`: `event: <ServerEvent::name()>`, `data:` — JSON
//! события. Первым приходит текущий `status_changed`; отставший подписчик
//! (переполнение шины) получает свежий `status_changed` вместо пропущенных.
//! Keep-alive — комментарий раз в 15 с. Поток закрывается при остановке сервиса.

use std::convert::Infallible;
use std::time::Duration;

use axum::extract::State;
use axum::response::sse::{Event, KeepAlive, Sse};
use focus_api_types::ServerEvent;
use futures_util::Stream;
use tokio::sync::broadcast::{Receiver, error::RecvError};

use crate::RestState;

/// Период keep-alive.
const KEEP_ALIVE: Duration = Duration::from_secs(15);

struct Feed {
    state: RestState,
    rx: Receiver<ServerEvent>,
    first: bool,
}

pub async fn events(
    State(state): State<RestState>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let rx = state.events.subscribe();
    let feed = Feed {
        state,
        rx,
        first: true,
    };
    let stream = futures_util::stream::unfold(feed, |mut f| async move {
        let ev = next(&mut f).await?;
        Some((Ok(to_sse(&ev)), f))
    });
    Sse::new(stream).keep_alive(KeepAlive::new().interval(KEEP_ALIVE).text("ping"))
}

async fn next(f: &mut Feed) -> Option<ServerEvent> {
    if std::mem::take(&mut f.first)
        && let Some(ev) = fresh_status(&f.state).await
    {
        return Some(ev);
    }
    loop {
        tokio::select! {
            () = f.state.shutdown.cancelled() => return None,
            r = f.rx.recv() => match r {
                Ok(ev) => return Some(ev),
                Err(RecvError::Closed) => return None,
                Err(RecvError::Lagged(n)) => {
                    tracing::debug!(skipped = n, "SSE: подписчик отстал");
                    if let Some(ev) = fresh_status(&f.state).await {
                        return Some(ev);
                    }
                }
            },
        }
    }
}

async fn fresh_status(state: &RestState) -> Option<ServerEvent> {
    match state.service.status().await {
        Ok(s) => Some(ServerEvent::StatusChanged(s)),
        Err(e) => {
            tracing::warn!(error = %e, "SSE: не удалось прочитать статус");
            None
        }
    }
}

fn to_sse(ev: &ServerEvent) -> Event {
    let data = serde_json::to_string(ev).unwrap_or_else(|e| {
        tracing::error!(error = %e, "SSE: не удалось сериализовать событие");
        "{}".to_owned()
    });
    Event::default().event(ev.name()).data(data)
}
