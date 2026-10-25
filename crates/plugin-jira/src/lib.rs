//! Плагин Jira: импорт задачи и строк Smart Checklist как пунктов,
//! отправка изменений пунктов (новый пункт → строка `- текст`, пункт готов →
//! `+ текст`, отметку сняли → `- текст`), worklog по закрытым сессиям работы
//! и синхронизация из Jira (название, новые строки, статусы строк).
//!
//! - Настройки — `SettingsDto.plugins["jira"]` ([`JiraSettings`]), токен —
//!   в секретах плагина (никогда в настройках и логах).
//! - Маршруты: `GET`/`PUT /api/plugins/jira/settings`, `POST /api/plugins/jira/test`,
//!   очередь отправки и `POST /api/plugins/jira/sync` (см. `routes`).
//! - MCP: `jira_import(url)`, `jira_sync()`, `jira_status()`.
//! - Включён по умолчанию, но без настройки безвреден: события
//!   пропускаются без сети, статус — «не настроен».

mod checklist;
mod client;
mod queue;
mod routes;
mod settings;
mod sync;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock, RwLock};
use std::time::Duration;

use anyhow::Context as _;
use async_trait::async_trait;
use focus_api_types::{EndReason, ServerEvent, SessionKind, TaskDto, TaskId, TaskStatus};
use focus_core::{Session, Task};
use focus_plugin_api::{
    CoreError, DomainEvent, ImportedTask, McpTool, Migrator, Plugin, PluginCtx, TaskSource,
    ToolOutput,
};
use serde_json::{Value, json};
use sqlx::Row as _;

use crate::queue::NewOp;

pub use checklist::{Line, Marker};
pub use client::{
    CHECKLIST_PROPERTY, Field, FieldSchema, Issue, Jira, JiraError, checklist_text, jira_time,
    pick_checklist_field, worklog_body,
};
pub use queue::{Op, OpKind};
pub use settings::{Conn, JiraSettings, Mode, TOKEN_SECRET, WorklogSettings, validate_base_url};
pub use sync::{SYNC_EVERY, SyncResult};

/// Id плагина.
pub const ID: &str = "jira";

static MIGRATOR: Migrator = sqlx::migrate!("./migrations");

/// Таймаут одного запроса к Jira.
const HTTP_TIMEOUT: Duration = Duration::from_secs(30);

/// Плагин Jira.
#[derive(Clone)]
pub struct JiraPlugin {
    inner: Arc<Inner>,
}

struct Inner {
    http: reqwest::Client,
    /// Последние настройки (для [`TaskSource::matches`] без БД).
    settings: RwLock<JiraSettings>,
    /// Итог автопоиска поля чек-листа: (base_url, id или «поля нет»).
    field: Mutex<Option<(String, Option<String>)>>,
    /// Контекст от сервера (для MCP-инструментов).
    ctx: OnceLock<PluginCtx>,
    /// Последняя ошибка обращения к Jira (сбрасывается успехом).
    last_error: Mutex<Option<String>>,
    /// «Не настроен» уже записано в лог (до следующей смены настроек).
    unconfigured_logged: AtomicBool,
    /// Отправка очереди — по одной за раз (кнопка и автоотправка); под ним
    /// же меняются отметки в очереди, чтобы не убрать отправляемую запись.
    flush: tokio::sync::Mutex<()>,
    /// Синхронизация из Jira — по одной за раз (кнопка и автосинхронизация).
    sync: tokio::sync::Mutex<()>,
    /// Итог последней синхронизации.
    last_sync: Mutex<Option<SyncResult>>,
}

/// Итог отправки очереди.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct FlushResult {
    /// Сколько операций прошло.
    pub sent: usize,
    /// Ошибка, на которой отправка остановилась.
    pub error: Option<String>,
}

impl Default for JiraPlugin {
    fn default() -> Self {
        Self::new()
    }
}

impl JiraPlugin {
    /// Плагин с HTTP-клиентом по умолчанию (таймаут 30 с).
    pub fn new() -> Self {
        let http = reqwest::Client::builder()
            .timeout(HTTP_TIMEOUT)
            .user_agent(concat!("focus-flow/", env!("CARGO_PKG_VERSION")))
            .build()
            .unwrap_or_default();
        Self {
            inner: Arc::new(Inner {
                http,
                settings: RwLock::default(),
                field: Mutex::default(),
                ctx: OnceLock::new(),
                last_error: Mutex::default(),
                unconfigured_logged: AtomicBool::new(false),
                flush: tokio::sync::Mutex::new(()),
                sync: tokio::sync::Mutex::new(()),
                last_sync: Mutex::default(),
            }),
        }
    }

    fn cached_settings(&self) -> JiraSettings {
        self.inner
            .settings
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    async fn load(&self, ctx: &PluginCtx) -> anyhow::Result<JiraSettings> {
        let v = ctx.settings().get().await?;
        JiraSettings::from_value(&v).map_err(anyhow::Error::msg)
    }

    /// Настройки и подключение; `Err(причина)` — не настроен.
    async fn conn(&self, ctx: &PluginCtx) -> anyhow::Result<(JiraSettings, Result<Conn, String>)> {
        let s = self.load(ctx).await?;
        let token = ctx.secrets().get(TOKEN_SECRET)?;
        let c = s.conn(token);
        Ok((s, c))
    }

    /// Id поля чек-листа «Checklists»: из настроек, из кэша или автопоиском
    /// (`GET /rest/api/2/field`). Поле нужно только для чтения, когда Jira не
    /// отдаёт свойство чек-листа, поэтому его отсутствие и ошибка автопоиска
    /// не мешают: `None`.
    async fn field(&self, s: &JiraSettings, jira: &Jira<'_>, conn: &Conn) -> Option<String> {
        if let Some(id) = s
            .checklist_field_id
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            return Some(id.to_owned());
        }
        if let Some((base, id)) = lock(&self.inner.field).clone()
            && base == conn.base
        {
            return id;
        }
        let fields = match jira.fields().await {
            Ok(f) => f,
            Err(e) => {
                tracing::warn!(error = %e, "не удалось получить поля Jira — чек-лист читается из свойства задачи");
                return None;
            }
        };
        let found = pick_checklist_field(&fields).map(|f| f.id.clone());
        match &found {
            Some(id) => tracing::info!(field = %id, "найдено поле чек-листа Jira"),
            None => tracing::info!(
                "в Jira нет текстового поля «Checklists» — чек-лист читается из свойства задачи"
            ),
        }
        *lock(&self.inner.field) = Some((conn.base.clone(), found.clone()));
        found
    }

    fn note<T>(&self, r: Result<T, JiraError>) -> Result<T, JiraError> {
        let mut le = lock(&self.inner.last_error);
        match &r {
            Ok(_) => *le = None,
            Err(e) => *le = Some(e.to_string()),
        }
        r
    }

    fn log_unconfigured(&self, why: &str) {
        if !self.inner.unconfigured_logged.swap(true, Ordering::SeqCst) {
            tracing::info!(reason = why, "Jira не настроена — события пропускаются");
        }
    }

    /// Ключ задачи Jira, связанной с нашей задачей верхнего уровня: из
    /// `jira_links`, иначе — по её `external_url` (и связь запоминается).
    async fn issue_of(
        &self,
        ctx: &PluginCtx,
        task: &Task,
        s: &JiraSettings,
    ) -> anyhow::Result<Option<String>> {
        let row = sqlx::query("SELECT issue_key FROM jira_links WHERE task_id = ?")
            .bind(task.id.to_string())
            .fetch_optional(ctx.pool())
            .await?;
        if let Some(r) = row {
            return Ok(Some(r.try_get("issue_key")?));
        }
        let Some(url) = &task.external_url else {
            return Ok(None);
        };
        let Some(key) = key_from_url(url, s.host().as_deref()) else {
            return Ok(None);
        };
        link(ctx, task.id, &key, url).await?;
        Ok(Some(key))
    }

    // ------------------------------------------------------------ события

    /// Событие → запись в очереди (без сети). С `auto_send` очередь сразу
    /// отправляется; ошибка отправки — ошибка события, и воркер outbox
    /// повторит его позже (запись в очереди та же, без дублей).
    async fn handle(&self, ev: &DomainEvent, ctx: &PluginCtx) -> anyhow::Result<()> {
        match ev {
            DomainEvent::TaskImported {
                source,
                task,
                created,
            } if source == ID => {
                return self.on_imported(ctx, task, *created).await;
            }
            DomainEvent::ItemCreated {
                task,
                parent: Some(parent),
            } => {
                self.enqueue_item(ctx, OpKind::ChecklistAdd, task, parent)
                    .await?
            }
            DomainEvent::ItemCompleted {
                task,
                parent: Some(parent),
            } => {
                self.enqueue_item(ctx, OpKind::ChecklistDone, task, parent)
                    .await?
            }
            DomainEvent::ItemReopened {
                task,
                parent: Some(parent),
            } => {
                self.enqueue_item(ctx, OpKind::ChecklistUndone, task, parent)
                    .await?
            }
            DomainEvent::SessionClosed {
                session,
                item,
                parent,
            } => {
                self.enqueue_worklog(ctx, session, item.as_ref(), parent.as_ref())
                    .await?
            }
            _ => {}
        }
        let (s, conn) = self.conn(ctx).await?;
        if s.auto_send && conn.is_ok() {
            let r = self.flush(ctx).await?;
            if let Some(e) = r.error {
                anyhow::bail!(e);
            }
        }
        Ok(())
    }

    /// Импорт: запомнить связь, название (новой задачи — оно из Jira) и
    /// строки пунктов с их статусом в Jira (без сети).
    async fn on_imported(&self, ctx: &PluginCtx, task: &Task, created: bool) -> anyhow::Result<()> {
        let s = self.load(ctx).await?;
        let Some(key) = self.issue_of(ctx, task, &s).await? else {
            return Ok(());
        };
        if created {
            sqlx::query("UPDATE jira_links SET title = COALESCE(title, ?) WHERE task_id = ?")
                .bind(&task.title)
                .bind(task.id.to_string())
                .execute(ctx.pool())
                .await?;
        }
        let tree = ctx.service()?.list_tasks().await?;
        if let Some(t) = find_task(&tree, task.id) {
            for c in &t.children {
                sqlx::query(
                    "INSERT OR IGNORE INTO jira_items (item_id, issue_key, line_text, jira_status) VALUES (?, ?, ?, ?)",
                )
                .bind(c.id.to_string())
                .bind(&key)
                .bind(&c.title)
                .bind(status_str(c.status))
                .execute(ctx.pool())
                .await?;
            }
        }
        Ok(())
    }

    /// Новый или готовый пункт задачи Jira → операция с чек-листом.
    async fn enqueue_item(
        &self,
        ctx: &PluginCtx,
        kind: OpKind,
        item: &Task,
        parent: &Task,
    ) -> anyhow::Result<()> {
        let (s, conn) = self.conn(ctx).await?;
        let Some(key) = self.issue_of(ctx, parent, &s).await? else {
            return Ok(());
        };
        if let Err(why) = conn {
            self.log_unconfigured(&why);
            return Ok(());
        }
        let id = item.id.to_string();
        if kind == OpKind::ChecklistAdd && item_line(ctx, &id).await?.is_some() {
            return Ok(()); // строка уже в чек-листе (импорт или повтор)
        }
        let op = NewOp {
            kind,
            ref_id: id,
            issue_key: key,
            text: item.title.clone(),
            worklog: None,
        };
        if kind.is_mark() {
            let _one = self.inner.flush.lock().await;
            match queue::push_mark(ctx.pool(), &op).await? {
                queue::Mark::Added => {
                    tracing::info!(issue = %op.issue_key, item = %item.title, kind = ?kind, "в очереди Jira");
                }
                queue::Mark::Cancelled => {
                    tracing::info!(issue = %op.issue_key, item = %item.title, "отметка и снятие отметки погасили друг друга");
                }
                queue::Mark::Same => {}
            }
        } else if queue::push(ctx.pool(), &op).await? {
            tracing::info!(issue = %op.issue_key, item = %item.title, kind = ?kind, "в очереди Jira");
        }
        Ok(())
    }

    /// Закрытая сессия → worklog по правилам из настроек.
    async fn enqueue_worklog(
        &self,
        ctx: &PluginCtx,
        session: &Session,
        item: Option<&Task>,
        parent: Option<&Task>,
    ) -> anyhow::Result<()> {
        let (s, conn) = self.conn(ctx).await?;
        let wanted = match (session.kind, session.end_reason) {
            (SessionKind::Work, Some(EndReason::Interrupted)) => s.worklog.interrupted,
            (SessionKind::Work, _) => s.worklog.work,
            (SessionKind::Planning, _) => s.worklog.planning,
            (SessionKind::Rest, _) => false,
        };
        if !wanted {
            return Ok(());
        }
        let Some(ended) = session.ended_at else {
            return Ok(());
        };
        let (parent, comment) = match session.kind {
            SessionKind::Work => match (item, parent) {
                (Some(i), Some(p)) => (p.clone(), i.title.clone()),
                _ => return Ok(()),
            },
            _ => match planning_target(ctx, session).await? {
                Some((p, title)) => (p, format!("Планирование: {title}")),
                None => return Ok(()),
            },
        };
        let Some(key) = self.issue_of(ctx, &parent, &s).await? else {
            return Ok(());
        };
        if let Err(why) = conn {
            self.log_unconfigured(&why);
            return Ok(());
        }
        let sid = session.id.to_string();
        if worklog_sent(ctx, &sid).await? {
            return Ok(());
        }
        let secs = (ended - session.started_at).num_seconds();
        let op = NewOp {
            kind: OpKind::Worklog,
            ref_id: sid,
            issue_key: key,
            text: comment,
            worklog: Some((session.started_at, secs)),
        };
        if queue::push(ctx.pool(), &op).await? {
            tracing::info!(issue = %op.issue_key, seconds = secs, "worklog в очереди Jira");
        }
        Ok(())
    }

    // ------------------------------------------------------------ отправка

    /// Отправить ждущие операции по порядку. Первая ошибка останавливает
    /// отправку (следующие могут зависеть от неё: пункт добавлен → готов) и
    /// записывается на операцию. `Err` — только БД или «не настроена».
    pub(crate) async fn flush(&self, ctx: &PluginCtx) -> anyhow::Result<FlushResult> {
        let _one = self.inner.flush.lock().await;
        let ops = queue::pending(ctx.pool()).await?;
        if ops.is_empty() {
            return Ok(FlushResult::default());
        }
        let (s, conn) = self.conn(ctx).await?;
        let conn =
            conn.map_err(|why| CoreError::Validation(format!("Jira не настроена: {why}")))?;
        let jira = Jira::new(&self.inner.http, &conn);
        let mut sent = 0;
        for op in ops {
            match self.send_op(ctx, &s, &jira, &conn, &op).await {
                Ok(()) => {
                    queue::mark_sent(ctx.pool(), op.id).await?;
                    sent += 1;
                }
                Err(e) => {
                    let msg = format!("{e:#}");
                    queue::mark_failed(ctx.pool(), op.id, &msg).await?;
                    tracing::warn!(issue = %op.issue_key, kind = ?op.kind, error = %msg, "не отправлено в Jira");
                    return Ok(FlushResult {
                        sent,
                        error: Some(format!("{}: {msg}", op.issue_key)),
                    });
                }
            }
        }
        Ok(FlushResult { sent, error: None })
    }

    async fn send_op(
        &self,
        ctx: &PluginCtx,
        s: &JiraSettings,
        jira: &Jira<'_>,
        conn: &Conn,
        op: &Op,
    ) -> anyhow::Result<()> {
        let key = &op.issue_key;
        match op.kind {
            OpKind::ChecklistAdd => {
                if item_line(ctx, &op.ref_id).await?.is_some() {
                    return Ok(()); // уже отправлен
                }
                let field = self.field(s, jira, conn).await;
                let issue = self.note(jira.issue(key, field.as_deref()).await)?;
                // Строка уже есть (её добавили в Jira или прошлая попытка
                // успела записать) — только запоминаем.
                let status = match checklist::find_line(&issue.checklist, &op.text) {
                    Some(idx) => checklist::status_at(&issue.checklist, idx),
                    None => {
                        let text = checklist::append(&issue.checklist, &op.text);
                        self.note(jira.put_checklist(issue.id_or_key(), &text).await)?;
                        tracing::info!(issue = %key, item = %op.text, "пункт добавлен в чек-лист Jira");
                        Some(TaskStatus::Planned)
                    }
                };
                remember_line(ctx, &op.ref_id, key, op.text.trim(), status).await?;
            }
            OpKind::ChecklistDone | OpKind::ChecklistUndone => {
                let done = op.kind == OpKind::ChecklistDone;
                let field = self.field(s, jira, conn).await;
                let text = item_line(ctx, &op.ref_id)
                    .await?
                    .unwrap_or_else(|| op.text.clone());
                // Перечитать чек-лист перед записью: строки могли поменять в Jira.
                let issue = self.note(jira.issue(key, field.as_deref()).await)?;
                let idx = checklist::find_line(&issue.checklist, &text)
                    .or_else(|| checklist::find_line(&issue.checklist, &op.text));
                let Some(idx) = idx else {
                    tracing::warn!(issue = %key, item = %op.text, "строка пункта не найдена в чек-листе Jira — пропущено");
                    return Ok(());
                };
                let changed = if done {
                    checklist::set_done(&issue.checklist, idx)
                } else {
                    checklist::set_todo(&issue.checklist, idx)
                };
                let status = match changed {
                    Some(new) => {
                        self.note(jira.put_checklist(issue.id_or_key(), &new).await)?;
                        tracing::info!(issue = %key, item = %op.text, done, "отметка пункта записана в Jira");
                        checklist::status_at(&new, idx)
                    }
                    None => checklist::status_at(&issue.checklist, idx),
                };
                set_line_status(ctx, &op.ref_id, status).await?;
            }
            OpKind::Worklog => {
                if worklog_sent(ctx, &op.ref_id).await? {
                    return Ok(());
                }
                let (Some(started), Some(secs)) = (op.started_at, op.seconds) else {
                    anyhow::bail!("в записи worklog нет времени");
                };
                let id = self.note(jira.add_worklog(key, started, secs, &op.text).await)?;
                sqlx::query(
                    "INSERT OR IGNORE INTO jira_worklogs (session_id, issue_key, worklog_id, status) VALUES (?, ?, ?, 'sent')",
                )
                .bind(&op.ref_id)
                .bind(key)
                .bind(&id)
                .execute(ctx.pool())
                .await?;
                tracing::info!(issue = %key, seconds = secs.max(60), "worklog записан в Jira");
            }
        }
        Ok(())
    }

    /// Очередь для `GET /queue`.
    pub(crate) async fn queue_json(&self, ctx: &PluginCtx) -> anyhow::Result<Value> {
        let s = self.load(ctx).await?;
        let items = queue::pending(ctx.pool()).await?;
        Ok(json!({"auto_send": s.auto_send, "items": items}))
    }

    // ------------------------------------------------------------ импорт

    async fn do_import(&self, url: &str, ctx: &PluginCtx) -> anyhow::Result<ImportedTask> {
        let (s, conn) = self.conn(ctx).await?;
        let conn =
            conn.map_err(|why| CoreError::PluginUnavailable(format!("Jira не настроена: {why}")))?;
        let key = key_from_input(url, s.host().as_deref()).ok_or_else(|| {
            CoreError::Validation(format!("«{url}» — не ссылка на задачу Jira и не ключ"))
        })?;
        let jira = Jira::new(&self.inner.http, &conn);
        let field = self.field(&s, &jira, &conn).await;
        let issue = match jira.issue(&key, field.as_deref()).await {
            Err(JiraError::Client { status: 404, .. }) => {
                return Err(
                    CoreError::Validation(format!("задача {key} не найдена в Jira")).into(),
                );
            }
            r => self.note(r)?,
        };
        let summary = issue.summary.trim();
        Ok(ImportedTask {
            source: ID.to_owned(),
            url: format!("{}/browse/{}", conn.base, issue.key),
            title: import_title(&issue.key, summary),
            notes: None,
            items: checklist::items(&issue.checklist),
        })
    }

    // ------------------------------------------------------------ состояние

    /// Состояние для `jira_status` и `GET /settings`.
    async fn state_json(&self, ctx: &PluginCtx) -> anyhow::Result<Value> {
        let (s, conn) = self.conn(ctx).await?;
        let has_token = ctx
            .secrets()
            .get(TOKEN_SECRET)?
            .is_some_and(|t| !t.is_empty());
        let field = s.checklist_field_id.clone().or_else(|| {
            lock(&self.inner.field)
                .as_ref()
                .filter(|(b, _)| Some(b) == s.base().as_ref())
                .and_then(|(_, id)| id.clone())
        });
        Ok(json!({
            "enabled": ctx.settings().enabled().await?,
            "configured": conn.is_ok(),
            "problem": conn.err(),
            "base_url": s.base(),
            "mode": s.mode,
            "email": s.email,
            "checklist_field_id": s.checklist_field_id,
            "checklist_field_resolved": field,
            "worklog": s.worklog,
            "has_token": has_token,
            "auto_send": s.auto_send,
            "auto_sync": s.auto_sync,
            "sync_every_min": SYNC_EVERY.as_secs() / 60,
            "last_sync": lock(&self.inner.last_sync).clone(),
            "queue_pending": queue::pending_count(ctx.pool()).await?,
            "last_error": lock(&self.inner.last_error).clone(),
        }))
    }

    fn ctx(&self) -> anyhow::Result<PluginCtx> {
        self.inner
            .ctx
            .get()
            .cloned()
            .context("плагин Jira ещё не подключён к сервису")
    }

    fn reset_cache(&self) {
        *lock(&self.inner.field) = None;
        self.inner
            .unconfigured_logged
            .store(false, Ordering::SeqCst);
    }
}

#[async_trait]
impl Plugin for JiraPlugin {
    fn id(&self) -> &'static str {
        ID
    }

    fn name(&self) -> &'static str {
        "Jira"
    }

    fn migrations(&self) -> Option<&'static Migrator> {
        Some(&MIGRATOR)
    }

    fn attach(&self, ctx: &PluginCtx) {
        let _ = self.inner.ctx.set(ctx.clone());
    }

    fn enabled_by_default(&self) -> bool {
        true
    }

    fn settings_changed(&self, settings: &Value) {
        let s = JiraSettings::from_value(settings).unwrap_or_else(|e| {
            tracing::warn!(error = %e, "настройки Jira не разобраны — используются умолчания");
            JiraSettings::default()
        });
        let mut cur = self
            .inner
            .settings
            .write()
            .unwrap_or_else(|e| e.into_inner());
        if *cur != s {
            *cur = s;
            drop(cur);
            self.reset_cache();
        }
    }

    async fn status_detail(&self, ctx: &PluginCtx) -> Option<String> {
        match self.conn(ctx).await {
            Ok((_, Ok(_))) => None,
            Ok((_, Err(why))) => Some(format!("не настроен: {why}")),
            Err(e) => Some(format!("не настроен: {e:#}")),
        }
    }

    async fn on_event(&self, event: &DomainEvent, ctx: &PluginCtx) -> anyhow::Result<()> {
        self.handle(event, ctx).await
    }

    async fn tick(&self, ctx: &PluginCtx) -> anyhow::Result<()> {
        self.auto_sync(ctx).await
    }

    fn routes(&self, ctx: PluginCtx) -> Option<axum::Router> {
        Some(routes::router(self.clone(), ctx))
    }

    fn mcp_tools(&self) -> Vec<McpTool> {
        let p = self.clone();
        let import = McpTool::new(
            "jira_import",
            "Импортировать задачу Jira по ссылке (https://<jira>/browse/KEY-1) или ключу (KEY-1): \
             создаёт задачу «KEY-1 · название» и пункты из строк Smart Checklist. Повторный \
             импорт не дублирует задачу, а догружает новые строки.",
            json!({
                "type": "object",
                "properties": {"url": {"type": "string", "description": "ссылка на задачу Jira или ключ"}},
                "required": ["url"],
                "additionalProperties": false,
            }),
            move |args| {
                let p = p.clone();
                async move {
                    let url = args
                        .get("url")
                        .and_then(Value::as_str)
                        .map(str::to_owned)
                        .ok_or_else(|| {
                            focus_api_types::ApiError::Validation("аргументы: нужен url".into())
                        })?;
                    let ctx = p.ctx()?;
                    let r = ctx.service()?.import(&url).await?;
                    ctx.broadcast(ServerEvent::TaskChanged { task_id: r.task.id });
                    let line = if r.created {
                        format!(
                            "Импортирована «{}»: пунктов {}.",
                            r.task.title, r.added_items
                        )
                    } else {
                        format!(
                            "«{}» уже была; новых пунктов: {}.",
                            r.task.title, r.added_items
                        )
                    };
                    ToolOutput::new(line, &r)
                }
            },
        );
        let p = self.clone();
        let sync = McpTool::new(
            "jira_sync",
            "Синхронизировать задачи с Jira: перечитать все связанные открытые задачи и \
             обновить у себя название, новые строки Smart Checklist и статусы строк, которые \
             поменяли в Jira. Неотправленные изменения пунктов у нас не трогаются.",
            json!({"type": "object", "properties": {}, "additionalProperties": false}),
            move |_| {
                let p = p.clone();
                async move {
                    let ctx = p.ctx()?;
                    let r = p.sync(&ctx).await?;
                    ToolOutput::new(r.summary(), &r)
                }
            },
        );
        let p = self.clone();
        let status = McpTool::new(
            "jira_status",
            "Состояние плагина Jira: настроен ли, base_url, режим, есть ли токен, поле чек-листа, \
             последняя ошибка. Токен не показывается.",
            json!({"type": "object", "properties": {}, "additionalProperties": false}),
            move |_| {
                let p = p.clone();
                async move {
                    let ctx = p.ctx()?;
                    let v = p.state_json(&ctx).await?;
                    let line = if v["configured"] == true {
                        format!(
                            "Jira настроена: {}.",
                            v["base_url"].as_str().unwrap_or_default()
                        )
                    } else {
                        format!(
                            "Jira не настроена: {}.",
                            v["problem"].as_str().unwrap_or_default()
                        )
                    };
                    ToolOutput::new(line, &v)
                }
            },
        );
        vec![import, sync, status]
    }
}

#[async_trait]
impl TaskSource for JiraPlugin {
    fn matches(&self, url: &str) -> bool {
        let s = self.cached_settings();
        s.base().is_some() && key_from_input(url, s.host().as_deref()).is_some()
    }

    async fn import(&self, url: &str, ctx: &PluginCtx) -> anyhow::Result<ImportedTask> {
        self.do_import(url, ctx).await
    }
}

// ---------------------------------------------------------------- помощники

/// Ключ задачи (`PROJ-12`): буква, затем буквы/цифры/`_`, `-`, номер.
pub fn parse_key(s: &str) -> Option<String> {
    let (proj, num) = s.rsplit_once('-')?;
    let mut pc = proj.chars();
    let ok = pc.next().is_some_and(|c| c.is_ascii_alphabetic())
        && pc.all(|c| c.is_ascii_alphanumeric() || c == '_')
        && !num.is_empty()
        && num.bytes().all(|b| b.is_ascii_digit());
    ok.then(|| s.to_ascii_uppercase())
}

/// Ключ из ссылки `https://<host>/…/browse/KEY-1` на хосте `host`.
pub fn key_from_url(url: &str, host: Option<&str>) -> Option<String> {
    let u = reqwest::Url::parse(url.trim()).ok()?;
    if !matches!(u.scheme(), "http" | "https") {
        return None;
    }
    let h = u.host_str()?.to_lowercase();
    if Some(h.as_str()) != host {
        return None;
    }
    let mut segs = u.path_segments()?;
    segs.by_ref().find(|s| *s == "browse")?;
    parse_key(segs.next()?)
}

/// Ключ из ссылки на хосте `host` или «голого» ключа `KEY-1` (заглавными).
pub fn key_from_input(input: &str, host: Option<&str>) -> Option<String> {
    let t = input.trim();
    if t.contains("://") {
        return key_from_url(t, host);
    }
    let upper = t.chars().next().is_some_and(|c| c.is_ascii_uppercase());
    if upper { parse_key(t) } else { None }
}

async fn link(ctx: &PluginCtx, task: TaskId, key: &str, url: &str) -> anyhow::Result<()> {
    sqlx::query(
        "INSERT INTO jira_links (task_id, issue_key, url) VALUES (?, ?, ?)
         ON CONFLICT (task_id) DO UPDATE SET issue_key = excluded.issue_key, url = excluded.url",
    )
    .bind(task.to_string())
    .bind(key)
    .bind(url)
    .execute(ctx.pool())
    .await?;
    Ok(())
}

/// Название задачи при импорте: «KEY · summary» (без summary — ключ).
pub(crate) fn import_title(key: &str, summary: &str) -> String {
    let summary = summary.trim();
    if summary.is_empty() {
        key.to_owned()
    } else {
        format!("{key} · {summary}")
    }
}

/// Статус в `jira_items.jira_status`.
pub(crate) fn status_str(s: TaskStatus) -> &'static str {
    match s {
        TaskStatus::Planned => "planned",
        TaskStatus::InProgress => "in_progress",
        TaskStatus::Done => "done",
        TaskStatus::Cancelled => "cancelled",
    }
}

pub(crate) fn parse_status(s: &str) -> Option<TaskStatus> {
    match s {
        "planned" => Some(TaskStatus::Planned),
        "in_progress" => Some(TaskStatus::InProgress),
        "done" => Some(TaskStatus::Done),
        "cancelled" => Some(TaskStatus::Cancelled),
        _ => None,
    }
}

/// Запомнить строку пункта и её статус в Jira (`None` — не знаем).
pub(crate) async fn remember_line(
    ctx: &PluginCtx,
    item: &str,
    key: &str,
    line: &str,
    status: Option<TaskStatus>,
) -> anyhow::Result<()> {
    sqlx::query(
        "INSERT INTO jira_items (item_id, issue_key, line_text, jira_status) VALUES (?, ?, ?, ?)
         ON CONFLICT (item_id) DO UPDATE SET issue_key = excluded.issue_key,
             line_text = excluded.line_text, jira_status = excluded.jira_status",
    )
    .bind(item)
    .bind(key)
    .bind(line)
    .bind(status.map(status_str))
    .execute(ctx.pool())
    .await?;
    Ok(())
}

/// Статус строки пункта в Jira после записи (строки нет в `jira_items` — ничего).
async fn set_line_status(
    ctx: &PluginCtx,
    item: &str,
    status: Option<TaskStatus>,
) -> anyhow::Result<()> {
    sqlx::query("UPDATE jira_items SET jira_status = ? WHERE item_id = ?")
        .bind(status.map(status_str))
        .bind(item)
        .execute(ctx.pool())
        .await?;
    Ok(())
}

async fn item_line(ctx: &PluginCtx, item: &str) -> anyhow::Result<Option<String>> {
    let row = sqlx::query("SELECT line_text FROM jira_items WHERE item_id = ?")
        .bind(item)
        .fetch_optional(ctx.pool())
        .await?;
    Ok(match row {
        Some(r) => Some(r.try_get("line_text")?),
        None => None,
    })
}

async fn worklog_sent(ctx: &PluginCtx, session: &str) -> anyhow::Result<bool> {
    let row = sqlx::query("SELECT 1 FROM jira_worklogs WHERE session_id = ?")
        .bind(session)
        .fetch_optional(ctx.pool())
        .await?;
    Ok(row.is_some())
}

fn find_task(tree: &[TaskDto], id: TaskId) -> Option<&TaskDto> {
    tree.iter().find_map(|t| {
        if t.id == id {
            Some(t)
        } else {
            find_task(&t.children, id)
        }
    })
}

/// Для планирования: задача последней работы этого дня до него и название пункта.
async fn planning_target(
    ctx: &PluginCtx,
    session: &Session,
) -> anyhow::Result<Option<(Task, String)>> {
    let svc = ctx.service()?;
    let date = session
        .started_at
        .with_timezone(&chrono::Local)
        .date_naive();
    let day = svc.day(date).await?;
    let Some(item_id) = day
        .sessions
        .iter()
        .filter(|x| x.kind == SessionKind::Work && x.started_at <= session.started_at)
        .max_by_key(|x| x.started_at)
        .and_then(|x| x.task_id)
    else {
        return Ok(None);
    };
    let tree = svc.list_tasks().await?;
    let Some(parent) = tree
        .iter()
        .find(|t| t.children.iter().any(|c| c.id == item_id))
    else {
        return Ok(None);
    };
    let title = parent
        .children
        .iter()
        .find(|c| c.id == item_id)
        .map(|c| c.title.clone())
        .unwrap_or_default();
    let task = Task {
        id: parent.id,
        parent_id: None,
        title: parent.title.clone(),
        notes: parent.notes.clone(),
        status: parent.status,
        planned_for: parent.planned_for,
        estimate_intervals: parent.estimate_intervals,
        sort_order: parent.sort_order,
        created_at: parent.created_at,
        done_at: parent.done_at,
        external_url: parent.external_url.clone(),
    };
    Ok(Some((task, title)))
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys() {
        assert_eq!(parse_key("PROJ-12").as_deref(), Some("PROJ-12"));
        assert_eq!(parse_key("ab_2-7").as_deref(), Some("AB_2-7"));
        for bad in ["PROJ", "-1", "1A-2", "PROJ-", "PR J-1", "PROJ-1a"] {
            assert!(parse_key(bad).is_none(), "{bad}");
        }
        let h = Some("jira.example.com");
        assert_eq!(
            key_from_input("https://jira.example.com/browse/ABC-1", h).as_deref(),
            Some("ABC-1")
        );
        assert_eq!(
            key_from_input("https://JIRA.example.com/jira/browse/abc-1?focused=1", h).as_deref(),
            Some("ABC-1")
        );
        assert!(key_from_input("https://other.com/browse/ABC-1", h).is_none());
        assert!(key_from_input("https://jira.example.com/projects/ABC", h).is_none());
        assert_eq!(key_from_input(" ABC-1 ", h).as_deref(), Some("ABC-1"));
        assert!(key_from_input("abc-1", h).is_none());
    }
}
