//! Лента событий для `get_status`: небольшое кольцо последних значимых
//! [`ServerEvent`] (закончилась сессия, ошибка плагина) с номерами и курсор
//! «что уже показано». Курсор общий на сервис: пользователь один, а MCP
//! работает без сессий (каждый запрос — новый обработчик).

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use focus_api_types::ServerEvent;
use tokio::sync::broadcast::{self, error::RecvError};
use tokio_util::sync::CancellationToken;

/// Сколько событий помнит лента.
pub const FEED_CAPACITY: usize = 64;

/// Событие ленты с номером.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FeedItem {
    /// Номер (растёт с 1).
    pub seq: u64,
    /// Событие.
    pub event: ServerEvent,
}

/// Новые события с прошлого вызова [`EventFeed::take_new`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NewEvents {
    /// События по порядку.
    pub items: Vec<FeedItem>,
    /// Сколько событий не поместилось в кольцо и потеряно.
    pub missed: u64,
}

#[derive(Debug, Default)]
struct State {
    buf: VecDeque<FeedItem>,
    last_seq: u64,
    cursor: u64,
}

/// Лента событий. Дёшево клонируется (`Arc`).
#[derive(Debug, Clone, Default)]
pub struct EventFeed(Arc<Mutex<State>>);

impl EventFeed {
    /// Пустая лента.
    pub fn new() -> Self {
        Self::default()
    }

    /// Попадает ли событие в ленту: только то, что модель могла пропустить —
    /// конец сессии и ошибки плагинов (не `status_changed`/`task_changed`).
    pub fn is_relevant(event: &ServerEvent) -> bool {
        matches!(
            event,
            ServerEvent::SessionEnded { .. } | ServerEvent::PluginError { .. }
        )
    }

    /// Добавить событие (нерелевантные пропускаются).
    pub fn push(&self, event: ServerEvent) {
        if !Self::is_relevant(&event) {
            return;
        }
        let mut st = self.lock();
        st.last_seq += 1;
        let seq = st.last_seq;
        st.buf.push_back(FeedItem { seq, event });
        while st.buf.len() > FEED_CAPACITY {
            st.buf.pop_front();
        }
    }

    /// События новее прошлого вызова; курсор сдвигается на последнее.
    pub fn take_new(&self) -> NewEvents {
        let mut st = self.lock();
        let cursor = st.cursor;
        let items: Vec<FeedItem> = st.buf.iter().filter(|i| i.seq > cursor).cloned().collect();
        let first = items.first().map_or(st.last_seq + 1, |i| i.seq);
        let missed = first.saturating_sub(cursor + 1);
        st.cursor = st.last_seq;
        NewEvents { items, missed }
    }

    /// Читать шину событий сервиса до отмены `cancel`.
    pub fn spawn(
        &self,
        mut rx: broadcast::Receiver<ServerEvent>,
        cancel: CancellationToken,
    ) -> tokio::task::JoinHandle<()> {
        let feed = self.clone();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    () = cancel.cancelled() => break,
                    r = rx.recv() => match r {
                        Ok(ev) => feed.push(ev),
                        Err(RecvError::Lagged(n)) => {
                            tracing::debug!(skipped = n, "лента MCP отстала от шины");
                        }
                        Err(RecvError::Closed) => break,
                    },
                }
            }
        })
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn err(n: u32) -> ServerEvent {
        ServerEvent::PluginError {
            plugin: "p".into(),
            message: n.to_string(),
        }
    }

    #[test]
    fn cursor_and_ring() {
        let f = EventFeed::new();
        assert_eq!(f.take_new(), NewEvents::default());
        f.push(ServerEvent::TaskChanged {
            task_id: focus_api_types::TaskId::from(focus_api_types::Uuid::nil()),
        });
        f.push(err(1));
        f.push(err(2));
        let n = f.take_new();
        assert_eq!(n.items.iter().map(|i| i.seq).collect::<Vec<_>>(), [1, 2]);
        assert_eq!(n.missed, 0);
        assert!(f.take_new().items.is_empty());

        for i in 0..(FEED_CAPACITY as u32 + 5) {
            f.push(err(i));
        }
        let n = f.take_new();
        assert_eq!(n.items.len(), FEED_CAPACITY);
        assert_eq!(n.missed, 5);
        assert_eq!(n.items[0].seq, 8);
    }
}
