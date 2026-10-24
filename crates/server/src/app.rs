//! [`App`] — всё, что нужно обработчикам и фоновым задачам: хранилище,
//! прикладной сервис, шина событий для SSE и токен доступа.

use std::sync::Arc;

use anyhow::Context as _;
use focus_api_types::ServerEvent;
use focus_core::{Clock, FocusService, NoTaskSources, SystemClock, TaskSources};
use focus_storage::Storage;
use tokio::sync::broadcast;

use crate::config::Config;
use crate::token::load_or_create_token;

/// Сколько событий держит шина для медленного подписчика SSE; отставший
/// пропускает старые (и получает свежий `status_changed` следующим).
const EVENTS_CAPACITY: usize = 256;

/// Собранный сервис. Дёшево клонируется.
#[derive(Clone)]
pub struct App {
    /// Хранилище SQLite.
    pub storage: Storage,
    /// Прикладной сервис ядра.
    pub service: Arc<FocusService>,
    /// Шина событий для клиентов (SSE).
    pub events: broadcast::Sender<ServerEvent>,
    /// Токен доступа (`Authorization: Bearer`).
    pub token: Arc<str>,
    /// Конфигурация, с которой собран сервис.
    pub config: Arc<Config>,
}

impl App {
    /// Открыть БД и токен из data dir конфига; системные часы, без
    /// источников задач (плагины — шаг 5).
    pub async fn open(config: Config) -> anyhow::Result<Self> {
        let storage = Storage::open(config.db_path())
            .await
            .with_context(|| format!("не удалось открыть БД {}", config.db_path().display()))?;
        let token = load_or_create_token(&config.token_path())?;
        Ok(Self::from_parts(
            config,
            storage,
            Arc::new(SystemClock),
            Arc::new(NoTaskSources),
            token,
        ))
    }

    /// Собрать из готовых частей (тесты: БД в памяти, управляемые часы).
    pub fn from_parts(
        config: Config,
        storage: Storage,
        clock: Arc<dyn Clock>,
        sources: Arc<dyn TaskSources>,
        token: impl Into<Arc<str>>,
    ) -> Self {
        let service = Arc::new(FocusService::new(storage.ports(clock, sources)));
        let (events, _) = broadcast::channel(EVENTS_CAPACITY);
        Self {
            storage,
            service,
            events,
            token: token.into(),
            config: Arc::new(config),
        }
    }

    /// Разослать событие подписчикам; если их нет — ничего не делает.
    pub fn broadcast(&self, event: ServerEvent) {
        // Ошибка `send` означает только «нет подписчиков».
        let _ = self.events.send(event);
    }
}
