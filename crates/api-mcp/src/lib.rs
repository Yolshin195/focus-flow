//! MCP-вход сервиса Focus Flow (`rmcp`, Streamable HTTP на `/mcp`):
//! инструменты ядра ([`core_tools`]) и `mcp_tools()` включённых плагинов.
//!
//! - Транспорт — Streamable HTTP без сессий (`legacy_session_mode = false`)
//!   с JSON-ответами: каждый POST — самостоятельный запрос. Проверки
//!   `Host`/`Origin`/токена делает тот же middleware, что и у REST
//!   (подключает `focus-server`); собственные проверки `rmcp` выключены,
//!   чтобы списки разрешённых хостов были в одном месте.
//! - Результат инструмента: первый текстовый блок — короткая сводка
//!   по-русски, второй — компактный JSON. Ошибки контракта — ошибка
//!   инструмента (`isError: true`) с текстом и `{"code", "details"}`.
//! - Команды публикуют те же события в шину SSE, что и REST.
//! - `get_status` отдаёт `events` — что случилось с прошлого вызова
//!   ([`EventFeed`]).

mod feed;
mod fmt;
mod tools;

use std::sync::Arc;

use axum::Router;
use focus_api_types::{ApiError, ServerEvent};
use focus_core::{CoreError, FocusService};
use focus_plugin_api::{McpTool, PluginTools, ToolOutput};
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, ErrorCode,
    Implementation, ListToolsResult, PaginatedRequestParams, ServerCapabilities, ServerConfig,
    Tool,
};
use rmcp::service::RequestContext;
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::streamable_http_server::{StreamableHttpServerConfig, StreamableHttpService};
use rmcp::{ErrorData, RoleServer, ServerHandler};
use serde_json::{Value, json};
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;

pub use feed::{EventFeed, FEED_CAPACITY, FeedItem, NewEvents};
pub use tools::core_tools;

/// Путь MCP в HTTP-сервисе.
pub const MCP_PATH: &str = "/mcp";

/// Инструкция для модели, отдаётся в `initialize`.
pub const INSTRUCTIONS: &str = "Focus Flow — трекер фокуса 50/10 пользователя. \
Ритм: работа 50 мин над одним пунктом → отдых 10 мин. Паузы нет: досрочно закончить работу можно \
только «Готово» (finish_item, пункт выполнен) или «Прервать» (interrupt, отвлёкся — плохо). \
Когда лимит работы истекает, сервис сам закрывает её и запускает отдых; отдых пропустить нельзя. \
Планирование (start_planning) идёт в норму дня, без лимита, заканчивается stop или стартом работы. \
Время без сессий после отдыха — потерянное. Норма дня (обычно 8 ч) = работа + планирование. \
В работу берётся пункт — узел без детей. Начинай с get_status: он показывает текущую сессию, \
сколько осталось и что случилось с прошлого вызова.";

/// Всё, что нужно инструментам MCP.
#[derive(Clone)]
pub struct McpState {
    /// Прикладной сервис ядра.
    pub service: Arc<FocusService>,
    /// Шина событий: команды публикуют сюда (как REST).
    pub events: broadcast::Sender<ServerEvent>,
    /// Инструменты включённых плагинов.
    pub plugins: Arc<dyn PluginTools>,
    /// Лента событий для `get_status`.
    pub feed: EventFeed,
    /// Остановка сервиса.
    pub shutdown: CancellationToken,
}

impl McpState {
    /// Состояние MCP; лента начинает слушать шину `events` (нужен runtime
    /// tokio) и останавливается по `shutdown`.
    pub fn new(
        service: Arc<FocusService>,
        events: broadcast::Sender<ServerEvent>,
        plugins: Arc<dyn PluginTools>,
        shutdown: CancellationToken,
    ) -> Self {
        let feed = EventFeed::new();
        feed.spawn(events.subscribe(), shutdown.clone());
        Self {
            service,
            events,
            plugins,
            feed,
            shutdown,
        }
    }

    pub(crate) fn broadcast(&self, event: ServerEvent) {
        // Ошибка — только «нет подписчиков».
        let _ = self.events.send(event);
    }

    /// Все инструменты: ядра + включённых плагинов (плагин не может
    /// перекрыть инструмент ядра).
    pub fn tools(&self) -> Vec<McpTool> {
        let mut all = core_tools(self);
        let plugin = self.plugins.tools();
        for t in plugin {
            if all.iter().any(|c| c.name == t.name) {
                tracing::warn!(tool = %t.name, "инструмент плагина совпадает с инструментом ядра — пропущен");
            } else {
                all.push(t);
            }
        }
        all
    }
}

/// Обработчик MCP (`rmcp::ServerHandler`).
#[derive(Clone)]
pub struct FocusMcp {
    state: McpState,
}

impl FocusMcp {
    /// Обработчик поверх состояния.
    pub fn new(state: McpState) -> Self {
        Self { state }
    }
}

fn to_rmcp(t: &McpTool) -> Tool {
    let schema = match &t.input_schema {
        Value::Object(m) => m.clone(),
        _ => serde_json::Map::from_iter([("type".to_owned(), json!("object"))]),
    };
    Tool::new(t.name.clone(), t.description.clone(), schema)
}

/// Ошибка инструмента → контракт: `CoreError`/`ApiError` как есть, прочее —
/// `internal`.
fn api_error(e: &anyhow::Error) -> ApiError {
    if let Some(c) = e.downcast_ref::<CoreError>() {
        return c.clone().into();
    }
    if let Some(a) = e.downcast_ref::<ApiError>() {
        return a.clone();
    }
    ApiError::Internal(format!("{e:#}"))
}

/// Результат вызова: сводка + компактный JSON (или ошибка инструмента).
pub fn tool_result(r: anyhow::Result<ToolOutput>) -> CallToolResult {
    match r {
        Ok(out) => CallToolResult::success(vec![
            ContentBlock::text(out.summary),
            ContentBlock::text(out.data.to_string()),
        ]),
        Err(e) => {
            let err = api_error(&e);
            let hint = fmt::error_hint(&err);
            let text = match hint {
                Some(h) => format!("Ошибка: {err}. {h}"),
                None => format!("Ошибка: {err}."),
            };
            let data = serde_json::to_string(&err).unwrap_or_default();
            CallToolResult::error(vec![ContentBlock::text(text), ContentBlock::text(data)])
        }
    }
}

impl ServerHandler for FocusMcp {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("focus-flow", env!("CARGO_PKG_VERSION")))
            .with_instructions(INSTRUCTIONS)
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        let tools = self.state.tools().iter().map(to_rmcp).collect();
        Ok(ListToolsResult::with_all_items(tools))
    }

    fn get_tool(&self, name: &str) -> Option<Tool> {
        self.state
            .tools()
            .iter()
            .find(|t| t.name == name)
            .map(to_rmcp)
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        let Some(tool) = self
            .state
            .tools()
            .into_iter()
            .find(|t| t.name == request.name)
        else {
            return Err(ErrorData::new(
                ErrorCode::METHOD_NOT_FOUND,
                format!("нет инструмента «{}»", request.name),
                None,
            ));
        };
        let args = request.arguments.map_or(Value::Null, Value::Object);
        tracing::debug!(tool = %tool.name, "MCP: вызов инструмента");
        let r = tool.call(args).await;
        if let Err(e) = &r {
            tracing::debug!(tool = %tool.name, error = %format!("{e:#}"), "MCP: ошибка инструмента");
        }
        Ok(tool_result(r).into())
    }
}

/// Сервис Streamable HTTP для `/mcp` (без проверок доступа — их добавляет
/// вызывающий).
pub fn service(state: McpState) -> StreamableHttpService<FocusMcp, LocalSessionManager> {
    let config = StreamableHttpServerConfig::default()
        .with_legacy_session_mode(false)
        .with_json_response(true)
        .with_sse_keep_alive(None)
        .disable_allowed_hosts()
        .disable_allowed_origins()
        .with_cancellation_token(state.shutdown.child_token());
    let handler = FocusMcp::new(state);
    StreamableHttpService::new(
        move || Ok(handler.clone()),
        Arc::new(LocalSessionManager::default()),
        config,
    )
}

/// Роутер с MCP на [`MCP_PATH`] (без проверок доступа).
pub fn router(state: McpState) -> Router {
    Router::new().route_service(MCP_PATH, service(state))
}
