//! [`App`] — всё, что нужно обработчикам и фоновым задачам: хранилище,
//! прикладной сервис, шина событий для SSE и токен доступа.

use std::sync::Arc;

use anyhow::Context as _;
use focus_api_types::ServerEvent;
use focus_core::{Clock, FocusService, SystemClock};
use focus_storage::Storage;
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;

use crate::config::Config;
use crate::plugins::{PluginRegistry, Registration, builtin_plugins};
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
    /// Реестр плагинов (он же источник задач и хозяин статуса плагинов).
    pub plugins: Arc<PluginRegistry>,
    /// Состояние MCP (лента событий для `get_status`).
    #[cfg(feature = "mcp")]
    pub mcp: focus_api_mcp::McpState,
    /// Конфигурация, с которой собран сервис.
    pub config: Arc<Config>,
    /// Остановка: таймер и потоки SSE завершаются по отмене.
    pub shutdown: CancellationToken,
}

impl App {
    /// Открыть БД и токен из data dir конфига; системные часы, плагины,
    /// собранные в бинарник ([`builtin_plugins`]).
    pub async fn open(config: Config) -> anyhow::Result<Self> {
        let storage = Storage::open(config.db_path())
            .await
            .with_context(|| format!("не удалось открыть БД {}", config.db_path().display()))?;
        let token = load_or_create_token(&config.token_path())?;
        Self::from_parts(
            config,
            storage,
            Arc::new(SystemClock),
            builtin_plugins(),
            token,
        )
        .await
    }

    /// Собрать из готовых частей (тесты: БД в памяти, управляемые часы,
    /// фейковые плагины). Применяет миграции плагинов и включает их по
    /// сохранённым настройкам.
    pub async fn from_parts(
        config: Config,
        storage: Storage,
        clock: Arc<dyn Clock>,
        plugins: Vec<Registration>,
        token: impl Into<Arc<str>>,
    ) -> anyhow::Result<Self> {
        let (events, _) = broadcast::channel(EVENTS_CAPACITY);
        let registry = Arc::new(
            PluginRegistry::new(
                &storage,
                clock.clone(),
                events.clone(),
                focus_plugin_api::ServiceSlot::new(),
                &config.secrets_dir(),
                plugins,
            )
            .await?,
        );
        let mut ports = storage.ports(clock, registry.clone());
        ports.plugins = registry.clone();
        let service = Arc::new(FocusService::new(ports));
        registry.service_slot().set(&service);
        let shutdown = CancellationToken::new();
        #[cfg(feature = "mcp")]
        let mcp = focus_api_mcp::McpState::new(
            service.clone(),
            events.clone(),
            registry.clone(),
            shutdown.clone(),
        );
        Ok(Self {
            storage,
            service,
            events,
            token: token.into(),
            plugins: registry,
            #[cfg(feature = "mcp")]
            mcp,
            config: Arc::new(config),
            shutdown,
        })
    }

    /// Разослать событие подписчикам; если их нет — ничего не делает.
    pub fn broadcast(&self, event: ServerEvent) {
        // Ошибка `send` означает только «нет подписчиков».
        let _ = self.events.send(event);
    }
}
