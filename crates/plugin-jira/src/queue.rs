//! Очередь отправки в Jira (`jira_queue`): операции, которые плагин
//! собирается выполнить. Обработчики событий только кладут сюда записи
//! (без сети); отправка — [`crate::JiraPlugin::flush`] по порядку id.

use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::{Row as _, SqlitePool};

/// Что сделать в Jira.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OpKind {
    /// Добавить строку `- текст` в чек-лист.
    ChecklistAdd,
    /// Отметить строку `+ текст`.
    ChecklistDone,
    /// Записать worklog.
    Worklog,
}

impl OpKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::ChecklistAdd => "checklist_add",
            Self::ChecklistDone => "checklist_done",
            Self::Worklog => "worklog",
        }
    }

    fn parse(s: &str) -> Option<Self> {
        match s {
            "checklist_add" => Some(Self::ChecklistAdd),
            "checklist_done" => Some(Self::ChecklistDone),
            "worklog" => Some(Self::Worklog),
            _ => None,
        }
    }
}

/// Новая операция.
#[derive(Debug, Clone)]
pub(crate) struct NewOp {
    pub kind: OpKind,
    /// Пункт или сессия.
    pub ref_id: String,
    pub issue_key: String,
    pub text: String,
    /// Worklog: начало и длительность.
    pub worklog: Option<(DateTime<Utc>, i64)>,
}

/// Операция в очереди (элемент ответа `GET /queue`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Op {
    /// Id записи.
    pub id: i64,
    /// Что сделать.
    pub kind: OpKind,
    /// Пункт (`checklist_*`) или сессия (`worklog`).
    pub ref_id: String,
    /// Задача Jira.
    pub issue_key: String,
    /// Текст пункта или комментарий worklog.
    pub text: String,
    /// Worklog: начало.
    pub started_at: Option<DateTime<Utc>>,
    /// Worklog: длительность, с.
    pub seconds: Option<i64>,
    /// Когда попала в очередь.
    pub created_at: DateTime<Utc>,
    /// Неудачных попыток.
    pub attempts: i64,
    /// Последняя ошибка.
    pub last_error: Option<String>,
    /// Когда была ошибка.
    pub last_error_at: Option<DateTime<Utc>>,
}

/// Положить операцию; повтор (тот же вид и пункт/сессия) — ничего, даже
/// если запись отправлена или удалена.
pub(crate) async fn push(pool: &SqlitePool, op: &NewOp) -> sqlx::Result<bool> {
    let (started, secs) = match op.worklog {
        Some((at, s)) => (Some(at.to_rfc3339()), Some(s)),
        None => (None, None),
    };
    let r = sqlx::query(
        "INSERT OR IGNORE INTO jira_queue (kind, ref_id, issue_key, text, started_at, seconds, created_at)
         VALUES (?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(op.kind.as_str())
    .bind(&op.ref_id)
    .bind(&op.issue_key)
    .bind(&op.text)
    .bind(started)
    .bind(secs)
    .bind(Utc::now().to_rfc3339())
    .execute(pool)
    .await?;
    Ok(r.rows_affected() > 0)
}

/// Ждущие операции по порядку.
pub(crate) async fn pending(pool: &SqlitePool) -> sqlx::Result<Vec<Op>> {
    let rows = sqlx::query(
        "SELECT id, kind, ref_id, issue_key, text, started_at, seconds, created_at, attempts,
                last_error, last_error_at
         FROM jira_queue WHERE status = 'pending' ORDER BY id",
    )
    .fetch_all(pool)
    .await?;
    let mut out = Vec::with_capacity(rows.len());
    for r in rows {
        let kind: String = r.try_get("kind")?;
        let Some(kind) = OpKind::parse(&kind) else {
            continue;
        };
        out.push(Op {
            id: r.try_get("id")?,
            kind,
            ref_id: r.try_get("ref_id")?,
            issue_key: r.try_get("issue_key")?,
            text: r.try_get("text")?,
            started_at: time(r.try_get("started_at")?),
            seconds: r.try_get("seconds")?,
            created_at: time(r.try_get("created_at")?).unwrap_or_default(),
            attempts: r.try_get("attempts")?,
            last_error: r.try_get("last_error")?,
            last_error_at: time(r.try_get("last_error_at")?),
        });
    }
    Ok(out)
}

/// Сколько ждут.
pub(crate) async fn pending_count(pool: &SqlitePool) -> sqlx::Result<i64> {
    sqlx::query_scalar("SELECT COUNT(*) FROM jira_queue WHERE status = 'pending'")
        .fetch_one(pool)
        .await
}

/// Отправлена.
pub(crate) async fn mark_sent(pool: &SqlitePool, id: i64) -> sqlx::Result<()> {
    sqlx::query("UPDATE jira_queue SET status = 'sent', last_error = NULL, last_error_at = NULL WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await
        .map(drop)
}

/// Не удалась.
pub(crate) async fn mark_failed(pool: &SqlitePool, id: i64, error: &str) -> sqlx::Result<()> {
    sqlx::query(
        "UPDATE jira_queue SET attempts = attempts + 1, last_error = ?, last_error_at = ? WHERE id = ?",
    )
    .bind(error)
    .bind(Utc::now().to_rfc3339())
    .bind(id)
    .execute(pool)
    .await
    .map(drop)
}

/// Убрать ждущую запись (пользователь решил не отправлять). `false` — её
/// нет или она уже не ждёт.
pub(crate) async fn drop_op(pool: &SqlitePool, id: i64) -> sqlx::Result<bool> {
    let r =
        sqlx::query("UPDATE jira_queue SET status = 'dropped' WHERE id = ? AND status = 'pending'")
            .bind(id)
            .execute(pool)
            .await?;
    Ok(r.rows_affected() > 0)
}

fn time(s: Option<String>) -> Option<DateTime<Utc>> {
    s.and_then(|s| DateTime::parse_from_rfc3339(&s).ok())
        .map(|t| t.with_timezone(&Utc))
}
