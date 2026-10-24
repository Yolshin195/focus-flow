//! MCP-инструменты плагина без зависимости от `rmcp`: имя, описание для
//! LLM, JSON Schema аргументов и асинхронный обработчик
//! `serde_json::Value → Result<ToolOutput>`. `focus-api-mcp` превращает их
//! в инструменты протокола.

use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use serde::Serialize;
use serde_json::Value;

/// Future обработчика инструмента.
pub type ToolFuture = Pin<Box<dyn Future<Output = anyhow::Result<ToolOutput>> + Send>>;

type Handler = dyn Fn(Value) -> ToolFuture + Send + Sync;

/// Результат инструмента: данные (JSON) и короткая строка для человека.
/// Клиенту MCP уходит текст `summary` + компактный JSON `data`.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolOutput {
    /// Короткое описание результата по-русски («Импортирована PROJ-1: 3 пункта»).
    pub summary: String,
    /// Данные результата.
    pub data: Value,
}

impl ToolOutput {
    /// Результат из сериализуемых данных.
    pub fn new(summary: impl Into<String>, data: &impl Serialize) -> anyhow::Result<Self> {
        Ok(Self {
            summary: summary.into(),
            data: serde_json::to_value(data)?,
        })
    }
}

/// MCP-инструмент плагина. Дёшево клонируется.
#[derive(Clone)]
pub struct McpTool {
    /// Имя инструмента (`<plugin_id>_<действие>`, `[a-z0-9_]`).
    pub name: String,
    /// Описание для LLM: что делает, когда вызывать.
    pub description: String,
    /// JSON Schema аргументов (`{"type": "object", ...}`).
    pub input_schema: Value,
    handler: Arc<Handler>,
}

impl McpTool {
    /// Инструмент с обработчиком `handler(arguments)`. Ошибка обработчика
    /// показывается модели как ошибка инструмента (`isError: true`).
    pub fn new<F, Fut>(
        name: impl Into<String>,
        description: impl Into<String>,
        input_schema: Value,
        handler: F,
    ) -> Self
    where
        F: Fn(Value) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = anyhow::Result<ToolOutput>> + Send + 'static,
    {
        Self {
            name: name.into(),
            description: description.into(),
            input_schema,
            handler: Arc::new(move |args| Box::pin(handler(args))),
        }
    }

    /// Вызвать инструмент; `arguments` — объект (или `Null`, если аргументов нет).
    pub fn call(&self, arguments: Value) -> ToolFuture {
        (self.handler)(arguments)
    }
}

impl fmt::Debug for McpTool {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("McpTool")
            .field("name", &self.name)
            .field("description", &self.description)
            .finish_non_exhaustive()
    }
}

/// Откуда MCP-вход берёт инструменты плагинов: реестр отдаёт инструменты
/// только включённых плагинов на момент вызова.
pub trait PluginTools: Send + Sync {
    /// Инструменты включённых плагинов.
    fn tools(&self) -> Vec<McpTool>;
}

/// Без плагинов.
impl PluginTools for () {
    fn tools(&self) -> Vec<McpTool> {
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn tool_calls_handler() {
        let t = McpTool::new(
            "fake_echo",
            "эхо",
            serde_json::json!({"type": "object"}),
            |args| async move { ToolOutput::new("ок", &args) },
        );
        let out = t.call(serde_json::json!({"x": 1})).await.unwrap();
        assert_eq!(out.summary, "ок");
        assert_eq!(out.data["x"], 1);
        assert!(format!("{t:?}").contains("fake_echo"));
    }
}
