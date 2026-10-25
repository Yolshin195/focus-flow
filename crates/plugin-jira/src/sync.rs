//! Синхронизация из Jira: свежее состояние всех связанных открытых задач —
//! название, новые строки чек-листа, статусы строк — применяется у нас через
//! [`FocusService::apply_source`](focus_plugin_api::FocusService::apply_source)
//! (без доменных событий, поэтому обратно в Jira ничего не уходит).
//!
//! Что поменяли в Jira, а что у нас, различается по базе — последнему
//! известному состоянию Jira (`jira_items.jira_status`, `jira_links.title`):
//! - строка: статус в Jira отличается от базы → пункт получает его; совпадает
//!   → пункт не трогаем (отмена или «в работе» у нас остаются). Пункт, у
//!   которого в очереди ждёт отправки операция, не трогаем: наше новее;
//! - название: Jira поменяла, а у нас прежнее → берём из Jira; переименованную
//!   у нас задачу не трогаем;
//! - новая строка → новый пункт в конце задачи; строку удалили в Jira — пункт
//!   остаётся.

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use chrono::{DateTime, Utc};
use focus_api_types::{ServerEvent, TaskDto, TaskId, TaskStatus};
use focus_plugin_api::{CoreError, PluginCtx, SourceUpdate};
use serde::Serialize;
use sqlx::Row as _;

use crate::checklist::{self, clean};
use crate::client::{Jira, JiraError};
use crate::{
    JiraPlugin, import_title, key_from_url, link, lock, parse_status, queue, remember_line,
};

/// Как часто синхронизировать сами, если включено `auto_sync`.
pub const SYNC_EVERY: Duration = Duration::from_secs(5 * 60);

/// Итог синхронизации (ответ `POST /sync`, `last_sync` в `GET /settings`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct SyncResult {
    /// Когда закончилась.
    pub at: DateTime<Utc>,
    /// Задач проверено.
    pub tasks: usize,
    /// Задач, в которых что-то изменилось.
    pub changed: usize,
    /// Задач, получивших новое название.
    pub renamed: usize,
    /// Новых пунктов.
    pub added_items: usize,
    /// Пунктов с новым статусом.
    pub updated_items: usize,
    /// Пунктов, изменённых в Jira, у которых в очереди ждёт наше изменение
    /// (не тронуты).
    pub kept_local: usize,
    /// Задач, которые не удалось получить.
    pub failed: usize,
    /// Первая ошибка.
    pub error: Option<String>,
}

impl SyncResult {
    /// Одна строка для MCP и логов.
    pub fn summary(&self) -> String {
        if self.tasks == 0 {
            return "Задач из Jira нет — синхронизировать нечего.".into();
        }
        let mut s = if self.changed == 0 {
            format!("Проверено задач: {}; изменений в Jira нет", self.tasks)
        } else {
            format!(
                "Проверено задач: {}; изменилось: {} (новых пунктов {}, статусов {}, переименовано {})",
                self.tasks, self.changed, self.added_items, self.updated_items, self.renamed
            )
        };
        if self.kept_local > 0 {
            s.push_str(&format!(
                "; не тронуто пунктов с неотправленными изменениями: {}",
                self.kept_local
            ));
        }
        if let Some(e) = &self.error {
            s.push_str(&format!("; не получено задач: {} — {e}", self.failed));
        }
        s.push('.');
        s
    }
}

/// Связь задачи с Jira.
struct Link {
    key: String,
    /// Название, последний раз взятое из Jira.
    title: Option<String>,
}

/// Что известно о строке пункта: текст и статус в Jira.
type Known = HashMap<TaskId, (String, Option<TaskStatus>)>;

impl JiraPlugin {
    /// Синхронизировать все связанные открытые задачи. Ошибка одной задачи
    /// не останавливает остальные (видна в итоге); `Err` — только «не
    /// настроена» и БД.
    pub(crate) async fn sync(&self, ctx: &PluginCtx) -> anyhow::Result<SyncResult> {
        let _one = self.inner.sync.lock().await;
        let (s, conn) = self.conn(ctx).await?;
        let conn =
            conn.map_err(|why| CoreError::Validation(format!("Jira не настроена: {why}")))?;
        let host = s.host();
        let mut targets = Vec::new();
        for task in ctx.service()?.list_tasks().await? {
            if matches!(task.status, TaskStatus::Done | TaskStatus::Cancelled) {
                continue;
            }
            if let Some(l) = link_of(ctx, &task, host.as_deref()).await? {
                targets.push((task, l));
            }
        }
        let mut r = SyncResult {
            tasks: targets.len(),
            ..SyncResult::default()
        };
        if !targets.is_empty() {
            let jira = Jira::new(&self.inner.http, &conn);
            let field = self.field(&s, &jira, &conn).await;
            for (task, l) in &targets {
                match self
                    .sync_task(ctx, &jira, field.as_deref(), task, l, &mut r)
                    .await
                {
                    Ok(true) => {
                        r.changed += 1;
                        ctx.broadcast(ServerEvent::TaskChanged { task_id: task.id });
                    }
                    Ok(false) => {}
                    Err(e) => {
                        let msg = format!("{}: {e:#}", l.key);
                        tracing::warn!(issue = %l.key, error = %format!("{e:#}"), "задача не синхронизирована с Jira");
                        r.failed += 1;
                        r.error.get_or_insert(msg);
                    }
                }
            }
        }
        r.at = Utc::now();
        tracing::info!(summary = %r.summary(), "синхронизация с Jira");
        *lock(&self.inner.last_sync) = Some(r.clone());
        Ok(r)
    }

    /// Автосинхронизация (из `tick`): включена, настроена и прошло
    /// [`SYNC_EVERY`] с прошлой (ручной или автоматической).
    pub(crate) async fn auto_sync(&self, ctx: &PluginCtx) -> anyhow::Result<()> {
        let (s, conn) = self.conn(ctx).await?;
        if !s.auto_sync || conn.is_err() {
            return Ok(());
        }
        let every = i64::try_from(SYNC_EVERY.as_secs()).unwrap_or(i64::MAX);
        let due = lock(&self.inner.last_sync)
            .as_ref()
            .is_none_or(|r| (Utc::now() - r.at).num_seconds() >= every);
        if due {
            self.sync(ctx).await?;
        }
        Ok(())
    }

    /// Одна задача; `true` — у нас что-то изменилось.
    async fn sync_task(
        &self,
        ctx: &PluginCtx,
        jira: &Jira<'_>,
        field: Option<&str>,
        task: &TaskDto,
        l: &Link,
        r: &mut SyncResult,
    ) -> anyhow::Result<bool> {
        let issue = match jira.issue(&l.key, field).await {
            Err(JiraError::Client { status: 404, .. }) => {
                anyhow::bail!("задача не найдена в Jira (удалена или нет прав)")
            }
            res => self.note(res)?,
        };
        let fresh = import_title(&issue.key, &issue.summary);
        let mut update = SourceUpdate {
            title: new_title(&task.title, l.title.as_deref(), &fresh, &issue.key),
            ..SourceUpdate::default()
        };

        let known = known_lines(ctx, task).await?;
        let mut used = HashSet::new();
        // Сопоставленные строки: пункт, текст и статус в Jira — новая база.
        let mut seen = Vec::new();
        for line in checklist::items(&issue.checklist) {
            let Some(item) = match_item(&task.children, &known, &used, &line.title) else {
                update.new_items.push(line);
                continue;
            };
            used.insert(item.id);
            seen.push((item.id, line.title.clone(), line.status));
            let base = known
                .get(&item.id)
                .and_then(|k| k.1)
                .unwrap_or_else(|| assumed_base(item.status, line.status));
            if line.status == base || item.status == line.status {
                continue;
            }
            if queue::has_pending(ctx.pool(), &item.id.to_string()).await? {
                r.kept_local += 1;
                continue;
            }
            update.statuses.push((item.id, line.status));
        }

        let mut changed = false;
        if update.title.is_some() || !update.statuses.is_empty() || !update.new_items.is_empty() {
            let applied = ctx.service()?.apply_source(task.id, update.clone()).await?;
            r.renamed += usize::from(applied.renamed);
            r.updated_items += applied.updated.len();
            changed = applied.renamed || !applied.updated.is_empty();
            for (line, id) in update.new_items.iter().zip(&applied.created) {
                if let Some(id) = id {
                    r.added_items += 1;
                    changed = true;
                    remember_line(
                        ctx,
                        &id.to_string(),
                        &issue.key,
                        &line.title,
                        Some(line.status),
                    )
                    .await?;
                }
            }
        }
        for (id, line, status) in seen {
            remember_line(ctx, &id.to_string(), &issue.key, &line, Some(status)).await?;
        }
        sqlx::query("UPDATE jira_links SET issue_key = ?, title = ? WHERE task_id = ?")
            .bind(&issue.key)
            .bind(&fresh)
            .bind(task.id.to_string())
            .execute(ctx.pool())
            .await?;
        Ok(changed)
    }
}

/// Связь задачи верхнего уровня с Jira: из `jira_links`, иначе — по её
/// `external_url` на хосте `host` (и связь запоминается).
async fn link_of(
    ctx: &PluginCtx,
    task: &TaskDto,
    host: Option<&str>,
) -> anyhow::Result<Option<Link>> {
    let row = sqlx::query("SELECT issue_key, title FROM jira_links WHERE task_id = ?")
        .bind(task.id.to_string())
        .fetch_optional(ctx.pool())
        .await?;
    if let Some(row) = row {
        return Ok(Some(Link {
            key: row.try_get("issue_key")?,
            title: row.try_get("title")?,
        }));
    }
    let Some(url) = &task.external_url else {
        return Ok(None);
    };
    let Some(key) = key_from_url(url, host) else {
        return Ok(None);
    };
    link(ctx, task.id, &key, url).await?;
    Ok(Some(Link { key, title: None }))
}

/// Строки пунктов задачи из `jira_items`.
async fn known_lines(ctx: &PluginCtx, task: &TaskDto) -> anyhow::Result<Known> {
    let mut out = Known::new();
    for c in &task.children {
        let row = sqlx::query("SELECT line_text, jira_status FROM jira_items WHERE item_id = ?")
            .bind(c.id.to_string())
            .fetch_optional(ctx.pool())
            .await?;
        if let Some(row) = row {
            let line: String = row.try_get("line_text")?;
            let status: Option<String> = row.try_get("jira_status")?;
            out.insert(c.id, (line, status.as_deref().and_then(parse_status)));
        }
    }
    Ok(out)
}

/// Пункт для строки `text` среди ещё не сопоставленных: по запомненному
/// тексту строки, затем по названию; сначала точно, затем без учёта регистра.
fn match_item<'a>(
    children: &'a [TaskDto],
    known: &Known,
    used: &HashSet<TaskId>,
    text: &str,
) -> Option<&'a TaskDto> {
    let free = || children.iter().filter(|c| !used.contains(&c.id));
    let line_of = |c: &TaskDto| known.get(&c.id).map(|k| clean(&k.0));
    let low = text.to_lowercase();
    free()
        .find(|c| line_of(c).as_deref() == Some(text))
        .or_else(|| free().find(|c| clean(&c.title) == text))
        .or_else(|| free().find(|c| line_of(c).is_some_and(|l| l.to_lowercase() == low)))
        .or_else(|| free().find(|c| clean(&c.title).to_lowercase() == low))
}

/// База для пункта, о котором неизвестно, что было в Jira (связь до этой
/// версии или пункт, созданный без Jira): готовый у нас — «готов»,
/// открытый — «сделать», отменённый — как в Jira сейчас (отмена у нас не
/// отправляется, и что в Jira, мы не знаем).
fn assumed_base(ours: TaskStatus, jira: TaskStatus) -> TaskStatus {
    match ours {
        TaskStatus::Done => TaskStatus::Done,
        TaskStatus::Planned | TaskStatus::InProgress => TaskStatus::Planned,
        TaskStatus::Cancelled => jira,
    }
}

/// Новое название задачи или `None`. `base` — название, последний раз взятое
/// из Jira; без него (связь до этой версии) название считается нетронутым,
/// пока оно в виде импорта: «KEY · …» или сам ключ.
fn new_title(ours: &str, base: Option<&str>, fresh: &str, key: &str) -> Option<String> {
    let untouched = match base {
        Some(b) if b == fresh => return None,
        Some(b) => ours == b,
        None => ours == key || ours.starts_with(&format!("{key} · ")),
    };
    (untouched && ours != fresh).then(|| fresh.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn title_follows_jira_unless_renamed() {
        let k = "ABC-1";
        assert_eq!(
            new_title("ABC-1 · old", Some("ABC-1 · old"), "ABC-1 · new", k).as_deref(),
            Some("ABC-1 · new")
        );
        assert_eq!(
            new_title("Моё", Some("ABC-1 · old"), "ABC-1 · new", k),
            None
        );
        assert_eq!(
            new_title("Моё", Some("ABC-1 · new"), "ABC-1 · new", k),
            None
        );
        // Связь до этой версии: только название в виде импорта.
        assert_eq!(
            new_title("ABC-1 · old", None, "ABC-1 · new", k).as_deref(),
            Some("ABC-1 · new")
        );
        assert_eq!(
            new_title("ABC-1", None, "ABC-1 · new", k).as_deref(),
            Some("ABC-1 · new")
        );
        assert_eq!(new_title("Моё ABC-1", None, "ABC-1 · new", k), None);
        assert_eq!(new_title("ABC-1 · new", None, "ABC-1 · new", k), None);
    }

    #[test]
    fn assumed_base_keeps_local_only_states() {
        use TaskStatus::*;
        assert_eq!(assumed_base(Done, Planned), Done);
        assert_eq!(assumed_base(InProgress, Planned), Planned);
        assert_eq!(assumed_base(Cancelled, Planned), Planned);
        assert_eq!(assumed_base(Cancelled, Done), Done);
    }

    #[test]
    fn summary_line() {
        let r = SyncResult {
            tasks: 3,
            changed: 1,
            added_items: 2,
            updated_items: 1,
            kept_local: 1,
            failed: 1,
            error: Some("ABC-9: 404".into()),
            ..SyncResult::default()
        };
        let s = r.summary();
        assert!(s.contains("Проверено задач: 3; изменилось: 1"), "{s}");
        assert!(s.contains("неотправленными изменениями: 1"), "{s}");
        assert!(s.contains("не получено задач: 1 — ABC-9: 404"), "{s}");
        assert!(SyncResult::default().summary().contains("нечего"));
    }
}
