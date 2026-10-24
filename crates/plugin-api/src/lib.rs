//! Плагинный API Focus Flow: трейты [`Plugin`] и [`TaskSource`],
//! [`DomainEvent`] (реэкспорт из ядра), контекст [`PluginCtx`] и абстракция
//! MCP-инструментов плагина ([`McpTool`]).
//!
//! Плагины подключаются в `focus-server` через реестр и cargo features —
//! без динамической загрузки. Включение — флаг `enabled` в настройках
//! плагина (`SettingsDto.plugins[<id>].enabled`).
//!
//! Что получает плагин:
//! - события ядра через outbox: [`Plugin::on_event`] вызывается воркером с
//!   повторами (экспоненциальная задержка), по порядку публикации;
//! - свои таблицы SQLite: [`Plugin::migrations`] (сервер применяет их при
//!   старте, журнал миграций — отдельная таблица `_sqlx_migrations_<id>`);
//! - свои HTTP-маршруты: [`Plugin::routes`] монтируются в `/api/plugins/<id>/`
//!   за теми же проверками `Host`/`Origin`/токена, что и REST;
//! - свои MCP-инструменты: [`Plugin::mcp_tools`] (без зависимости от `rmcp`);
//! - [`PluginCtx`]: сервис ядра, настройки, секреты, пул SQLite, логгер.

mod ctx;
mod mcp;
mod secrets;

use async_trait::async_trait;

pub use ctx::{PluginCtx, PluginSettings, ServiceSlot};
pub use focus_core::{CoreError, DomainEvent, FocusService, ImportedItem, ImportedTask};
pub use mcp::{McpTool, PluginTools, ToolFuture, ToolOutput};
pub use secrets::Secrets;
pub use sqlx::migrate::Migrator;

/// Плагин: реагирует на доменные события и может добавлять таблицы,
/// HTTP-маршруты и MCP-инструменты.
#[async_trait]
pub trait Plugin: Send + Sync + 'static {
    /// Уникальный id: `[a-z0-9_]+`, не `status` (например, `jira`). Префикс
    /// имён таблиц плагина и его MCP-инструментов.
    fn id(&self) -> &'static str;

    /// Человекочитаемое название (для логов и UI).
    fn name(&self) -> &'static str {
        self.id()
    }

    /// Сервер собрал контекст плагина (один раз, при регистрации). Плагин
    /// может сохранить его — например, для обработчиков [`Plugin::mcp_tools`].
    fn attach(&self, ctx: &PluginCtx) {
        let _ = ctx;
    }

    /// Включён ли плагин, пока в его настройках нет флага `enabled`
    /// (по умолчанию — выключен). Плагин, включённый по умолчанию, обязан
    /// быть безвредным без настройки: не ходить в сеть и не копить ошибки.
    fn enabled_by_default(&self) -> bool {
        false
    }

    /// Настройки плагина (`SettingsDto.plugins[<id>]`, `Null` — их нет)
    /// изменились или прочитаны при старте. Вызывается синхронно, чтобы
    /// плагин мог обновить кэш (например, для [`TaskSource::matches`]).
    fn settings_changed(&self, settings: &serde_json::Value) {
        let _ = settings;
    }

    /// Короткая строка состояния для `/api/plugins/status` (`detail`),
    /// например «не настроен». Без сети: вызывается на каждый запрос статуса.
    async fn status_detail(&self, ctx: &PluginCtx) -> Option<String> {
        let _ = ctx;
        None
    }

    /// Миграции таблиц плагина (имена таблиц — с префиксом `<id>_`), обычно
    /// `static MIGRATOR: Migrator = sqlx::migrate!("./migrations")`. Сервер
    /// применяет их при старте с отдельным журналом `_sqlx_migrations_<id>`,
    /// поэтому номера версий не пересекаются с миграциями ядра.
    fn migrations(&self) -> Option<&'static Migrator> {
        None
    }

    /// Обработать доменное событие. Ошибка → повтор позже (то же событие);
    /// следующие события этого плагина ждут, пока это не пройдёт. Поэтому
    /// обработчик должен быть идемпотентным, а «невозможное» событие (плохие
    /// данные, неизвестный тип) — пропускать с `Ok(())`, а не ошибкой.
    async fn on_event(&self, event: &DomainEvent, ctx: &PluginCtx) -> anyhow::Result<()>;

    /// HTTP-маршруты плагина; сервер вкладывает их в `/api/plugins/<id>`
    /// (маршрут `/settings` плагина → `/api/plugins/<id>/settings`). Пока
    /// плагин выключен, на них отвечается `503 plugin_unavailable`.
    fn routes(&self, ctx: PluginCtx) -> Option<axum::Router> {
        let _ = ctx;
        None
    }

    /// MCP-инструменты плагина (имена — с префиксом `<id>_`, например
    /// `jira_import`). Видны клиенту MCP, только пока плагин включён.
    fn mcp_tools(&self) -> Vec<McpTool> {
        Vec::new()
    }
}

/// Источник задач для `POST /api/import`: по ссылке/ключу отдаёт задачу с
/// пунктами. Реестр выбирает первый включённый источник, у которого
/// [`TaskSource::matches`] вернул `true`.
#[async_trait]
pub trait TaskSource: Plugin {
    /// Подходит ли ссылка (или ключ, например `PROJ-123`) этому источнику.
    /// Должна быть дешёвой: без сети.
    fn matches(&self, url: &str) -> bool;

    /// Загрузить задачу. `ImportedTask.url` — каноническая ссылка: по ней
    /// повторный импорт находит уже созданную задачу. Чтобы вернуть
    /// клиенту конкретную ошибку контракта, верните [`CoreError`]
    /// (`anyhow::Error::from(CoreError::Validation(..))`); любая другая
    /// ошибка станет `plugin_unavailable`.
    async fn import(&self, url: &str, ctx: &PluginCtx) -> anyhow::Result<ImportedTask>;
}

/// Проверка id плагина: `[a-z0-9_]+`, не длиннее 32, не `status`.
pub fn valid_plugin_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 32
        && id != "status"
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}

#[cfg(test)]
mod tests {
    use super::valid_plugin_id;

    #[test]
    fn plugin_ids() {
        for ok in ["jira", "fake_1"] {
            assert!(valid_plugin_id(ok), "{ok}");
        }
        for bad in ["", "status", "Jira", "a-b", "a/b", &"x".repeat(33)] {
            assert!(!valid_plugin_id(bad), "{bad}");
        }
    }
}
