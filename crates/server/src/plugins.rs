//! Реестр плагинов: регистрация (cargo features), миграции таблиц плагинов,
//! включение по настройкам, доставка outbox с повторами, статус,
//! маршрутизация импорта по [`TaskSource::matches`], маршруты и
//! MCP-инструменты включённых плагинов.
//!
//! Реестр реализует порты ядра [`PluginHost`] и [`TaskSources`] и
//! [`PluginTools`] для MCP.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::Path;
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use anyhow::Context as _;
use async_trait::async_trait;
use axum::Router;
use axum::extract::{Request, State};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use chrono::{DateTime, Utc};
use focus_api_rest::RestError;
use focus_api_types::{ApiError, PluginStatusDto, ServerEvent};
use focus_core::{
    Clock, CoreError, CoreResult, ImportedTask, Outbox, OutboxEntry, PluginHost, Settings,
    SettingsRepo, TaskSources,
};
use focus_plugin_api::{
    McpTool, Migrator, Plugin, PluginCtx, PluginTools, ServiceSlot, TaskSource, valid_plugin_id,
};
use focus_storage::Storage;
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;
use tracing::Instrument as _;

/// Сколько записей outbox забирать за проход.
const BATCH: usize = 32;
/// Предел времени обработки одного события плагином.
const EVENT_TIMEOUT: Duration = Duration::from_secs(60);
/// Как часто воркер outbox проверяет очередь.
pub const OUTBOX_POLL: Duration = Duration::from_secs(1);
/// Как часто звать [`Plugin::tick`] включённых плагинов.
pub const PLUGIN_TICK: Duration = Duration::from_secs(60);
/// Предел времени одного [`Plugin::tick`].
const TICK_TIMEOUT: Duration = Duration::from_secs(300);

/// Плагин для регистрации: обычный или источник задач.
#[derive(Clone)]
pub enum Registration {
    /// Плагин без импорта.
    Plugin(Arc<dyn Plugin>),
    /// Плагин — источник задач (`POST /api/import`).
    Source(Arc<dyn TaskSource>),
}

impl Registration {
    fn plugin(&self) -> Arc<dyn Plugin> {
        match self {
            Self::Plugin(p) => p.clone(),
            Self::Source(s) => s.clone(),
        }
    }
}

/// Плагины, собранные в этот бинарник (cargo features).
pub fn builtin_plugins() -> Vec<Registration> {
    vec![
        #[cfg(feature = "jira")]
        Registration::Source(Arc::new(focus_plugin_jira::JiraPlugin::new())),
    ]
}

/// Итог последней доставки (в памяти процесса).
#[derive(Debug, Clone, Default)]
struct Health {
    /// `Some(true)` — последняя доставка прошла, `Some(false)` — упала,
    /// `None` — доставок с запуска не было.
    last_ok: Option<bool>,
    last_error: Option<String>,
    last_error_at: Option<DateTime<Utc>>,
}

struct Entry {
    plugin: Arc<dyn Plugin>,
    source: Option<Arc<dyn TaskSource>>,
    ctx: PluginCtx,
    health: Mutex<Health>,
}

/// Реестр плагинов сервиса.
pub struct PluginRegistry {
    entries: Vec<Entry>,
    enabled: RwLock<BTreeSet<&'static str>>,
    outbox: Arc<dyn Outbox>,
    clock: Arc<dyn Clock>,
    events: broadcast::Sender<ServerEvent>,
    service: ServiceSlot,
}

impl PluginRegistry {
    /// Собрать реестр: проверить id, применить миграции плагинов, включить
    /// плагины по сохранённым настройкам и задать подписчиков outbox.
    /// Сервис ядра привязывается позже — [`ServiceSlot::set`] у `service`.
    pub async fn new(
        storage: &Storage,
        clock: Arc<dyn Clock>,
        events: broadcast::Sender<ServerEvent>,
        service: ServiceSlot,
        secrets_root: &Path,
        registrations: Vec<Registration>,
    ) -> anyhow::Result<Self> {
        let mut ids = HashSet::new();
        let mut entries = Vec::new();
        for r in registrations {
            let plugin = r.plugin();
            let id = plugin.id();
            anyhow::ensure!(valid_plugin_id(id), "недопустимый id плагина «{id}»");
            anyhow::ensure!(ids.insert(id), "плагин «{id}» зарегистрирован дважды");
            if let Some(m) = plugin.migrations() {
                run_migrations(storage, id, m).await?;
            }
            let source = match r {
                Registration::Source(s) => Some(s),
                Registration::Plugin(_) => None,
            };
            let ctx = PluginCtx::new(id, service.clone(), storage.pool().clone(), secrets_root)
                .with_enabled_default(plugin.enabled_by_default())
                .with_events(events.clone());
            plugin.attach(&ctx);
            entries.push(Entry {
                ctx,
                plugin,
                source,
                health: Mutex::default(),
            });
        }
        let reg = Self {
            entries,
            enabled: RwLock::default(),
            outbox: Arc::new(storage.clone()),
            clock,
            events,
            service,
        };
        let settings = SettingsRepo::load(storage)
            .await
            .context("не удалось прочитать настройки")?;
        reg.apply_settings(&settings).await?;
        tracing::info!(
            registered = ?reg.entries.iter().map(|e| e.plugin.id()).collect::<Vec<_>>(),
            enabled = ?reg.enabled_ids(),
            "плагины"
        );
        Ok(reg)
    }

    /// Ссылка на сервис ядра для контекстов плагинов.
    pub fn service_slot(&self) -> &ServiceSlot {
        &self.service
    }

    /// Id зарегистрированных плагинов.
    pub fn ids(&self) -> Vec<&'static str> {
        self.entries.iter().map(|e| e.plugin.id()).collect()
    }

    /// Id включённых плагинов.
    pub fn enabled_ids(&self) -> Vec<String> {
        read(&self.enabled)
            .iter()
            .map(|s| (*s).to_owned())
            .collect()
    }

    /// Включён ли плагин.
    pub fn is_enabled(&self, id: &str) -> bool {
        read(&self.enabled).contains(id)
    }

    fn entry(&self, id: &str) -> Option<&Entry> {
        self.entries.iter().find(|e| e.plugin.id() == id)
    }

    async fn apply_settings(&self, settings: &Settings) -> CoreResult<()> {
        for e in &self.entries {
            let v = settings
                .plugins
                .get(e.plugin.id())
                .cloned()
                .unwrap_or(serde_json::Value::Null);
            e.plugin.settings_changed(&v);
        }
        let enabled: BTreeSet<&'static str> = self
            .entries
            .iter()
            .filter(|e| plugin_enabled_or(settings, e.plugin.id(), e.plugin.enabled_by_default()))
            .map(|e| e.plugin.id())
            .collect();
        let ids: Vec<String> = enabled.iter().map(|s| (*s).to_owned()).collect();
        let changed = {
            let mut cur = self.enabled.write().unwrap_or_else(|e| e.into_inner());
            let changed = *cur != enabled;
            *cur = enabled;
            changed
        };
        self.outbox.set_subscribers(&ids).await?;
        if changed {
            tracing::info!(enabled = ?ids, "включённые плагины изменились");
        }
        Ok(())
    }

    /// Один проход доставки: готовые записи включённых плагинов → `on_event`
    /// → `mark_done` или `mark_failed` (повтор с задержкой) и
    /// `ServerEvent::PluginError`. После неудачи следующие записи того же
    /// плагина в этом проходе не трогаются (порядок). Возвращает число
    /// обработанных записей (доставленных и неудачных).
    pub async fn deliver_due(&self) -> CoreResult<usize> {
        let enabled = self.enabled_ids();
        if enabled.is_empty() {
            return Ok(0);
        }
        let now = self.clock.now();
        let due = self.outbox.fetch_due(now, &enabled, BATCH).await?;
        let mut failed: HashSet<String> = HashSet::new();
        let mut handled = 0;
        for e in due {
            if failed.contains(&e.plugin_id) || !self.is_enabled(&e.plugin_id) {
                continue;
            }
            let Some(entry) = self.entry(&e.plugin_id) else {
                continue;
            };
            handled += 1;
            match deliver(entry, &e).await {
                Ok(()) => {
                    self.outbox.mark_done(e.id, self.clock.now()).await?;
                    let mut h = lock(&entry.health);
                    h.last_ok = Some(true);
                }
                Err(err) => {
                    let msg = format!("{err:#}");
                    let at = self.clock.now();
                    let next = self.outbox.mark_failed(e.id, &msg, at).await?;
                    tracing::warn!(
                        plugin = %e.plugin_id,
                        event = e.event.name(),
                        attempt = e.attempts + 1,
                        retry_at = %next,
                        error = %msg,
                        "плагин не обработал событие"
                    );
                    {
                        let mut h = lock(&entry.health);
                        h.last_ok = Some(false);
                        h.last_error = Some(msg.clone());
                        h.last_error_at = Some(at);
                    }
                    let _ = self.events.send(ServerEvent::PluginError {
                        plugin: e.plugin_id.clone(),
                        message: msg,
                    });
                    failed.insert(e.plugin_id.clone());
                }
            }
        }
        Ok(handled)
    }

    /// Один проход [`Plugin::tick`] по включённым плагинам, по очереди.
    /// Ошибка плагина пишется в лог и не мешает остальным.
    pub async fn tick(&self) {
        for e in &self.entries {
            let id = e.plugin.id();
            if !self.is_enabled(id) {
                continue;
            }
            let fut = e.plugin.tick(&e.ctx);
            match tokio::time::timeout(TICK_TIMEOUT, fut)
                .instrument(e.ctx.span().clone())
                .await
            {
                Ok(Ok(())) => {}
                Ok(Err(err)) => {
                    tracing::warn!(plugin = id, error = %format!("{err:#}"), "фоновая работа плагина не удалась");
                }
                Err(_) => {
                    tracing::warn!(
                        plugin = id,
                        "фоновая работа плагина дольше {} с",
                        TICK_TIMEOUT.as_secs()
                    );
                }
            }
        }
    }

    /// Маршруты плагинов, вложенные в `/api/plugins/<id>`; пока плагин
    /// выключен — `503 plugin_unavailable`. Проверки доступа добавляет
    /// вызывающий (`focus-server::router`).
    pub fn routes(self: &Arc<Self>) -> Router {
        let mut router = Router::new();
        for e in &self.entries {
            let id = e.plugin.id();
            if let Some(r) = e.plugin.routes(e.ctx.clone()) {
                let gate =
                    axum::middleware::from_fn_with_state((self.clone(), id), require_enabled);
                router = router.nest(&format!("/api/plugins/{id}"), r.layer(gate));
            }
        }
        router
    }
}

async fn require_enabled(
    State((reg, id)): State<(Arc<PluginRegistry>, &'static str)>,
    req: Request,
    next: Next,
) -> Response {
    if reg.is_enabled(id) {
        next.run(req).await
    } else {
        RestError(ApiError::PluginUnavailable(format!(
            "плагин «{id}» выключен"
        )))
        .into_response()
    }
}

async fn deliver(entry: &Entry, e: &OutboxEntry) -> anyhow::Result<()> {
    let fut = entry.plugin.on_event(&e.event, &entry.ctx);
    match tokio::time::timeout(EVENT_TIMEOUT, fut)
        .instrument(entry.ctx.span().clone())
        .await
    {
        Ok(r) => r,
        Err(_) => anyhow::bail!("обработка события дольше {} с", EVENT_TIMEOUT.as_secs()),
    }
}

/// Включён ли плагин в настройках: `plugins[<id>].enabled == true`
/// (без флага — выключен).
pub fn plugin_enabled(settings: &Settings, id: &str) -> bool {
    plugin_enabled_or(settings, id, false)
}

/// Как [`plugin_enabled`], но без флага `enabled` — `default`
/// ([`Plugin::enabled_by_default`]).
pub fn plugin_enabled_or(settings: &Settings, id: &str, default: bool) -> bool {
    settings
        .plugins
        .get(id)
        .and_then(|v| v.get("enabled"))
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(default)
}

async fn run_migrations(storage: &Storage, id: &str, m: &Migrator) -> anyhow::Result<()> {
    let mut own = Migrator::with_migrations(m.iter().cloned().collect());
    // Свой журнал: версии плагина не пересекаются с версиями ядра.
    own.dangerous_set_table_name(format!("_sqlx_migrations_{id}"));
    own.run(storage.pool())
        .await
        .with_context(|| format!("миграции плагина «{id}»"))?;
    Ok(())
}

#[async_trait]
impl PluginHost for PluginRegistry {
    async fn status(&self) -> CoreResult<Vec<PluginStatusDto>> {
        let stats: BTreeMap<String, _> = self
            .outbox
            .stats()
            .await?
            .into_iter()
            .map(|s| (s.plugin_id.clone(), s))
            .collect();
        let mut out = Vec::with_capacity(self.entries.len());
        for e in &self.entries {
            let detail = e.plugin.status_detail(&e.ctx).await;
            out.push({
                let id = e.plugin.id();
                let h = lock(&e.health).clone();
                let st = stats.get(id);
                // Ошибка в очереди переживает перезапуск; ошибка в памяти —
                // последняя с запуска (запись могла уже пройти повтором).
                // Без ошибок — состояние от плагина («не настроен»), без времени.
                let (last_error, last_error_at) = match st.and_then(|s| s.last_error.clone()) {
                    Some(err) => (Some(err), st.and_then(|s| s.last_error_at)),
                    None if h.last_error.is_some() => (h.last_error.clone(), h.last_error_at),
                    None => (detail, None),
                };
                let queued_error = st.is_some_and(|s| s.last_error.is_some());
                PluginStatusDto {
                    id: id.to_owned(),
                    enabled: self.is_enabled(id),
                    healthy: h.last_ok.unwrap_or(!queued_error),
                    outbox_pending: st.map_or(0, |s| s.pending),
                    last_error,
                    last_error_at,
                }
            });
        }
        Ok(out)
    }

    async fn settings_changed(&self, settings: &Settings) -> CoreResult<()> {
        self.apply_settings(settings).await
    }
}

#[async_trait]
impl TaskSources for PluginRegistry {
    async fn import(&self, url: &str) -> CoreResult<ImportedTask> {
        let entry = self
            .entries
            .iter()
            .filter(|e| self.is_enabled(e.plugin.id()))
            .find(|e| e.source.as_ref().is_some_and(|s| s.matches(url)));
        let Some((entry, source)) = entry.and_then(|e| e.source.as_ref().map(|s| (e, s))) else {
            return Err(CoreError::PluginUnavailable(format!(
                "нет включённого источника задач для «{url}»"
            )));
        };
        let id = entry.plugin.id();
        match source
            .import(url, &entry.ctx)
            .instrument(entry.ctx.span().clone())
            .await
        {
            Ok(mut t) => {
                t.source = id.to_owned();
                Ok(t)
            }
            Err(e) => match e.downcast::<CoreError>() {
                Ok(core) => Err(core),
                Err(e) => {
                    tracing::warn!(plugin = id, error = %format!("{e:#}"), "импорт не удался");
                    Err(CoreError::PluginUnavailable(format!("{id}: {e:#}")))
                }
            },
        }
    }
}

impl PluginTools for PluginRegistry {
    fn tools(&self) -> Vec<McpTool> {
        self.entries
            .iter()
            .filter(|e| self.is_enabled(e.plugin.id()))
            .flat_map(|e| e.plugin.mcp_tools())
            .collect()
    }
}

/// Воркер outbox до отмены `cancel`: проход доставки, затем сон `poll`
/// (если очередь не выбрана целиком — сразу следующий проход).
pub async fn run_outbox_worker(
    registry: Arc<PluginRegistry>,
    poll: Duration,
    cancel: CancellationToken,
) {
    tracing::debug!("воркер outbox запущен");
    loop {
        let sleep = match registry.deliver_due().await {
            Ok(n) if n >= BATCH => Duration::ZERO,
            Ok(_) => poll,
            Err(e) => {
                tracing::error!(error = %e, "воркер outbox: ошибка прохода");
                poll
            }
        };
        tokio::select! {
            () = cancel.cancelled() => break,
            () = tokio::time::sleep(sleep) => {}
        }
    }
    tracing::debug!("воркер outbox остановлен");
}

/// Фоновая работа плагинов до отмены `cancel`: [`PluginRegistry::tick`]
/// раз в `every` (первый проход — через `every` после старта).
pub async fn run_plugin_ticker(
    registry: Arc<PluginRegistry>,
    every: Duration,
    cancel: CancellationToken,
) {
    tracing::debug!("фоновая работа плагинов запущена");
    loop {
        tokio::select! {
            () = cancel.cancelled() => break,
            () = tokio::time::sleep(every) => {}
        }
        tokio::select! {
            () = cancel.cancelled() => break,
            () = registry.tick() => {}
        }
    }
    tracing::debug!("фоновая работа плагинов остановлена");
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn read<T>(m: &RwLock<T>) -> std::sync::RwLockReadGuard<'_, T> {
    m.read().unwrap_or_else(|e| e.into_inner())
}
