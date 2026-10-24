//! [`PluginCtx`] — всё, что сервер даёт плагину: сервис ядра, настройки
//! плагина, секреты, пул SQLite и логгер.

use std::path::Path;
use std::sync::{Arc, OnceLock, Weak};

use focus_core::FocusService;
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;
use sqlx::SqlitePool;

use crate::secrets::Secrets;

/// Ссылка на сервис ядра, которую сервер заполняет после сборки
/// [`FocusService`] (сервису, в свою очередь, нужен реестр плагинов —
/// поэтому ссылка слабая и ставится позже).
#[derive(Debug, Clone, Default)]
pub struct ServiceSlot(Arc<OnceLock<Weak<FocusService>>>);

impl ServiceSlot {
    /// Пустая ссылка.
    pub fn new() -> Self {
        Self::default()
    }

    /// Привязать сервис (повторный вызов ничего не меняет).
    pub fn set(&self, service: &Arc<FocusService>) {
        let _ = self.0.set(Arc::downgrade(service));
    }

    /// Сервис, если он привязан и ещё жив.
    pub fn get(&self) -> Option<Arc<FocusService>> {
        self.0.get().and_then(Weak::upgrade)
    }
}

/// Контекст плагина. Дёшево клонируется.
#[derive(Debug, Clone)]
pub struct PluginCtx {
    id: &'static str,
    service: ServiceSlot,
    pool: SqlitePool,
    secrets: Secrets,
    span: tracing::Span,
}

impl PluginCtx {
    /// Контекст плагина `id`: секреты — в `<secrets_root>/<id>/`.
    pub fn new(
        id: &'static str,
        service: ServiceSlot,
        pool: SqlitePool,
        secrets_root: &Path,
    ) -> Self {
        Self {
            id,
            service,
            pool,
            secrets: Secrets::new(secrets_root.join(id)),
            span: tracing::info_span!("plugin", id),
        }
    }

    /// Id плагина.
    pub fn id(&self) -> &'static str {
        self.id
    }

    /// Сервис ядра: создавать задачи и пункты, читать статус и дерево.
    /// Ошибка — только если сервис ещё не собран или уже остановлен.
    pub fn service(&self) -> anyhow::Result<Arc<FocusService>> {
        self.service
            .get()
            .ok_or_else(|| anyhow::anyhow!("сервис ядра недоступен"))
    }

    /// Настройки плагина (`SettingsDto.plugins[<id>]`).
    pub fn settings(&self) -> PluginSettings {
        PluginSettings { ctx: self.clone() }
    }

    /// Секреты плагина (токены): файлы с правами `0600` в data dir.
    pub fn secrets(&self) -> &Secrets {
        &self.secrets
    }

    /// Пул SQLite — для таблиц плагина (`<id>_*`).
    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    /// Span `plugin{id=…}` для логов: `let _g = ctx.span().enter();` или
    /// `.instrument(ctx.span().clone())`. Воркер outbox уже вызывает
    /// [`crate::Plugin::on_event`] внутри него.
    pub fn span(&self) -> &tracing::Span {
        &self.span
    }
}

/// Настройки одного плагина — JSON в `SettingsDto.plugins[<id>]`; схему
/// знает плагин. Флаг `enabled` (bool) читает сервер: включён ли плагин.
#[derive(Debug, Clone)]
pub struct PluginSettings {
    ctx: PluginCtx,
}

impl PluginSettings {
    /// JSON настроек; `Null`, если их ещё нет.
    pub async fn get(&self) -> anyhow::Result<Value> {
        let all = self.ctx.service()?.settings().await?;
        Ok(all.plugins.get(self.ctx.id).cloned().unwrap_or(Value::Null))
    }

    /// Настройки как тип `T`; если их нет — `T::default()`.
    pub async fn get_as<T: DeserializeOwned + Default>(&self) -> anyhow::Result<T> {
        match self.get().await? {
            Value::Null => Ok(T::default()),
            v => Ok(serde_json::from_value(v)?),
        }
    }

    /// Включён ли плагин (`enabled: true` в его настройках).
    pub async fn enabled(&self) -> anyhow::Result<bool> {
        Ok(self
            .get()
            .await?
            .get("enabled")
            .and_then(Value::as_bool)
            .unwrap_or(false))
    }

    /// Заменить настройки плагина целиком (остальные настройки сервиса не
    /// меняются). Секреты сюда не кладут — для них [`PluginCtx::secrets`].
    pub async fn set(&self, value: &impl Serialize) -> anyhow::Result<()> {
        let svc = self.ctx.service()?;
        let mut all = svc.settings().await?;
        all.plugins
            .insert(self.ctx.id.to_owned(), serde_json::to_value(value)?);
        svc.update_settings(all).await?;
        Ok(())
    }
}
