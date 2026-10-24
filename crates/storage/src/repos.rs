//! Реализации портов ядра на SQLite.

use async_trait::async_trait;
use chrono::{DateTime, NaiveDate, Utc};
use focus_api_types::{DayId, SessionId, TaskId};
use focus_core::{
    CoreError, CoreResult, DayRepo, DomainEvent, EventSink, Outbox, OutboxEntry, OutboxId,
    OutboxStats, Session, SessionRepo, Settings, SettingsRepo, Task, TaskRepo, WorkDay,
};
use sqlx::sqlite::SqliteRow;

use crate::Storage;
use crate::codec::{RowExt, date, enum_str, ts, ts_opt};
use crate::error::{StorageError, StorageResult};

fn lock_read<T: Clone>(l: &std::sync::RwLock<T>) -> T {
    l.read().unwrap_or_else(|e| e.into_inner()).clone()
}

// ------------------------------------------------------------------ дни

const DAY_COLS: &str = "id, date, opened_at, closed_at, target_sec";

fn day_from(r: &SqliteRow) -> StorageResult<WorkDay> {
    Ok(WorkDay {
        id: r.id("id")?,
        date: r.date("date")?,
        opened_at: r.ts("opened_at")?,
        closed_at: r.ts_opt("closed_at")?,
        target_sec: r.int("target_sec")?,
    })
}

impl Storage {
    async fn day_where(&self, cond: &'static str, arg: String) -> StorageResult<Option<WorkDay>> {
        let sql = format!("SELECT {DAY_COLS} FROM work_days WHERE {cond}");
        sqlx::query(sqlx::AssertSqlSafe(sql))
            .bind(arg)
            .fetch_optional(&self.pool)
            .await?
            .as_ref()
            .map(day_from)
            .transpose()
    }

    async fn save_day(&self, d: &WorkDay) -> StorageResult<()> {
        sqlx::query(
            "INSERT INTO work_days (id, date, opened_at, closed_at, target_sec)
             VALUES (?, ?, ?, ?, ?)
             ON CONFLICT(id) DO UPDATE SET
                date = excluded.date, opened_at = excluded.opened_at,
                closed_at = excluded.closed_at, target_sec = excluded.target_sec",
        )
        .bind(d.id.to_string())
        .bind(date(d.date))
        .bind(ts(d.opened_at))
        .bind(ts_opt(d.closed_at))
        .bind(d.target_sec)
        .execute(&self.pool)
        .await?;
        Ok(())
    }
}

#[async_trait]
impl DayRepo for Storage {
    async fn get(&self, id: DayId) -> CoreResult<Option<WorkDay>> {
        Ok(self.day_where("id = ?", id.to_string()).await?)
    }

    async fn by_date(&self, d: NaiveDate) -> CoreResult<Option<WorkDay>> {
        Ok(self.day_where("date = ?", date(d)).await?)
    }

    async fn save(&self, day: &WorkDay) -> CoreResult<()> {
        Ok(self.save_day(day).await?)
    }
}

// ------------------------------------------------------------------ задачи

const TASK_COLS: &str = "id, parent_id, title, notes, status, planned_for, estimate_intervals, \
                         sort_order, created_at, done_at, external_url";
/// Порядок дерева — как в ядре: `(sort_order, created_at, id)`.
const TASK_ORDER: &str = "ORDER BY sort_order, created_at, id";

fn task_from(r: &SqliteRow) -> StorageResult<Task> {
    let estimate = r
        .int_opt("estimate_intervals")?
        .map(|v| {
            u32::try_from(v)
                .map_err(|_| StorageError::Decode(format!("estimate_intervals: {v} вне u32")))
        })
        .transpose()?;
    Ok(Task {
        id: r.id("id")?,
        parent_id: r.id_opt("parent_id")?,
        title: r.text("title")?,
        notes: r.text_opt("notes")?,
        status: r.enum_of("status")?,
        planned_for: r.date_opt("planned_for")?,
        estimate_intervals: estimate,
        sort_order: r.int("sort_order")?,
        created_at: r.ts("created_at")?,
        done_at: r.ts_opt("done_at")?,
        external_url: r.text_opt("external_url")?,
    })
}

impl Storage {
    async fn tasks_where(&self, cond: &str, arg: Option<String>) -> StorageResult<Vec<Task>> {
        let sql = format!("SELECT {TASK_COLS} FROM tasks WHERE {cond} {TASK_ORDER}");
        let mut q = sqlx::query(sqlx::AssertSqlSafe(sql));
        if let Some(a) = arg {
            q = q.bind(a);
        }
        q.fetch_all(&self.pool)
            .await?
            .iter()
            .map(task_from)
            .collect()
    }

    async fn save_task(&self, t: &Task) -> StorageResult<()> {
        sqlx::query(
            "INSERT INTO tasks (id, parent_id, title, notes, status, planned_for,
                                estimate_intervals, sort_order, created_at, done_at, external_url)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT(id) DO UPDATE SET
                parent_id = excluded.parent_id, title = excluded.title,
                notes = excluded.notes, status = excluded.status,
                planned_for = excluded.planned_for,
                estimate_intervals = excluded.estimate_intervals,
                sort_order = excluded.sort_order, created_at = excluded.created_at,
                done_at = excluded.done_at, external_url = excluded.external_url",
        )
        .bind(t.id.to_string())
        .bind(t.parent_id.map(|p| p.to_string()))
        .bind(&t.title)
        .bind(&t.notes)
        .bind(enum_str(&t.status)?)
        .bind(t.planned_for.map(date))
        .bind(t.estimate_intervals.map(i64::from))
        .bind(t.sort_order)
        .bind(ts(t.created_at))
        .bind(ts_opt(t.done_at))
        .bind(&t.external_url)
        .execute(&self.pool)
        .await?;
        Ok(())
    }
}

#[async_trait]
impl TaskRepo for Storage {
    async fn get(&self, id: TaskId) -> CoreResult<Option<Task>> {
        Ok(self
            .tasks_where("id = ?", Some(id.to_string()))
            .await?
            .pop())
    }

    async fn list(&self) -> CoreResult<Vec<Task>> {
        Ok(self.tasks_where("1 = 1", None).await?)
    }

    async fn children(&self, parent: Option<TaskId>) -> CoreResult<Vec<Task>> {
        Ok(match parent {
            Some(p) => {
                self.tasks_where("parent_id = ?", Some(p.to_string()))
                    .await?
            }
            None => self.tasks_where("parent_id IS NULL", None).await?,
        })
    }

    async fn by_external_url(&self, url: &str) -> CoreResult<Option<Task>> {
        Ok(self
            .tasks_where("external_url = ?", Some(url.to_owned()))
            .await?
            .into_iter()
            .next())
    }

    async fn save(&self, task: &Task) -> CoreResult<()> {
        Ok(self.save_task(task).await?)
    }
}

// ------------------------------------------------------------------ сессии

const SESSION_COLS: &str = "id, day_id, kind, task_id, started_at, ended_at, limit_sec, end_reason";

fn session_from(r: &SqliteRow) -> StorageResult<Session> {
    Ok(Session {
        id: r.id("id")?,
        day_id: r.id("day_id")?,
        kind: r.enum_of("kind")?,
        task_id: r.id_opt("task_id")?,
        started_at: r.ts("started_at")?,
        ended_at: r.ts_opt("ended_at")?,
        limit_sec: r.int_opt("limit_sec")?,
        end_reason: r.enum_opt("end_reason")?,
    })
}

impl Storage {
    async fn sessions_where(&self, cond: &str, arg: Option<String>) -> StorageResult<Vec<Session>> {
        let sql =
            format!("SELECT {SESSION_COLS} FROM sessions WHERE {cond} ORDER BY started_at, id");
        let mut q = sqlx::query(sqlx::AssertSqlSafe(sql));
        if let Some(a) = arg {
            q = q.bind(a);
        }
        q.fetch_all(&self.pool)
            .await?
            .iter()
            .map(session_from)
            .collect()
    }

    async fn save_session(&self, s: &Session) -> StorageResult<()> {
        sqlx::query(
            "INSERT INTO sessions (id, day_id, kind, task_id, started_at, ended_at,
                                   limit_sec, end_reason)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT(id) DO UPDATE SET
                day_id = excluded.day_id, kind = excluded.kind, task_id = excluded.task_id,
                started_at = excluded.started_at, ended_at = excluded.ended_at,
                limit_sec = excluded.limit_sec, end_reason = excluded.end_reason",
        )
        .bind(s.id.to_string())
        .bind(s.day_id.to_string())
        .bind(enum_str(&s.kind)?)
        .bind(s.task_id.map(|t| t.to_string()))
        .bind(ts(s.started_at))
        .bind(ts_opt(s.ended_at))
        .bind(s.limit_sec)
        .bind(s.end_reason.as_ref().map(enum_str).transpose()?)
        .execute(&self.pool)
        .await?;
        Ok(())
    }
}

#[async_trait]
impl SessionRepo for Storage {
    async fn get(&self, id: SessionId) -> CoreResult<Option<Session>> {
        Ok(self
            .sessions_where("id = ?", Some(id.to_string()))
            .await?
            .pop())
    }

    async fn active(&self) -> CoreResult<Option<Session>> {
        Ok(self
            .sessions_where("ended_at IS NULL", None)
            .await?
            .into_iter()
            .next())
    }

    async fn by_day(&self, day_id: DayId) -> CoreResult<Vec<Session>> {
        Ok(self
            .sessions_where("day_id = ?", Some(day_id.to_string()))
            .await?)
    }

    async fn save(&self, session: &Session) -> CoreResult<()> {
        Ok(self.save_session(session).await?)
    }
}

// ------------------------------------------------------------------ настройки

/// Ключи таблицы `settings`; значения — JSON.
const KEY_WORK: &str = "work_min";
const KEY_REST: &str = "rest_min";
const KEY_TARGET: &str = "daily_target_min";
const KEY_PLUGINS: &str = "plugins";

impl Storage {
    async fn load_settings(&self) -> StorageResult<Settings> {
        let rows = sqlx::query("SELECT key, value FROM settings")
            .fetch_all(&self.pool)
            .await?;
        // Неизвестные ключи игнорируются, отсутствующие — по умолчанию.
        let mut s = Settings::default();
        for r in &rows {
            let key = r.text("key")?;
            let value = r.text("value")?;
            match key.as_str() {
                KEY_WORK => s.work_min = serde_json::from_str(&value)?,
                KEY_REST => s.rest_min = serde_json::from_str(&value)?,
                KEY_TARGET => s.daily_target_min = serde_json::from_str(&value)?,
                KEY_PLUGINS => s.plugins = serde_json::from_str(&value)?,
                _ => {}
            }
        }
        Ok(s)
    }

    async fn save_settings(&self, s: &Settings) -> StorageResult<()> {
        let pairs = [
            (KEY_WORK, serde_json::to_string(&s.work_min)?),
            (KEY_REST, serde_json::to_string(&s.rest_min)?),
            (KEY_TARGET, serde_json::to_string(&s.daily_target_min)?),
            (KEY_PLUGINS, serde_json::to_string(&s.plugins)?),
        ];
        let mut tx = self.pool.begin().await?;
        for (k, v) in pairs {
            sqlx::query(
                "INSERT INTO settings (key, value) VALUES (?, ?)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            )
            .bind(k)
            .bind(v)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }
}

#[async_trait]
impl SettingsRepo for Storage {
    async fn load(&self) -> CoreResult<Settings> {
        Ok(self.load_settings().await?)
    }

    async fn save(&self, settings: &Settings) -> CoreResult<()> {
        Ok(self.save_settings(settings).await?)
    }
}

// ------------------------------------------------------------------ outbox

const OUTBOX_COLS: &str =
    "id, plugin_id, event_json, attempts, next_attempt_at, last_error, last_error_at, done_at";

fn outbox_from(r: &SqliteRow) -> StorageResult<OutboxEntry> {
    let attempts = r.int("attempts")?;
    Ok(OutboxEntry {
        id: r.int("id")?,
        plugin_id: r.text("plugin_id")?,
        event: serde_json::from_str(&r.text("event_json")?)?,
        attempts: u32::try_from(attempts)
            .map_err(|_| StorageError::Decode(format!("attempts: {attempts}")))?,
        next_attempt_at: r.ts_opt("next_attempt_at")?,
        last_error: r.text_opt("last_error")?,
        last_error_at: r.ts_opt("last_error_at")?,
        done_at: r.ts_opt("done_at")?,
    })
}

impl Storage {
    /// Задать подписчиков outbox (то же, что [`Outbox::set_subscribers`]).
    pub fn set_outbox_subscribers(&self, plugin_ids: &[String]) {
        *self.subscribers.write().unwrap_or_else(|e| e.into_inner()) = plugin_ids.to_vec();
    }

    async fn publish_events(&self, events: &[DomainEvent]) -> StorageResult<()> {
        let subscribers = lock_read(&self.subscribers);
        if events.is_empty() || subscribers.is_empty() {
            return Ok(());
        }
        let mut tx = self.pool.begin().await?;
        for ev in events {
            let json = serde_json::to_string(ev)?;
            for plugin_id in &subscribers {
                sqlx::query("INSERT INTO outbox (event_json, plugin_id) VALUES (?, ?)")
                    .bind(&json)
                    .bind(plugin_id)
                    .execute(&mut *tx)
                    .await?;
            }
        }
        tx.commit().await?;
        Ok(())
    }

    async fn outbox_entry(&self, id: OutboxId) -> StorageResult<Option<OutboxEntry>> {
        let sql = format!("SELECT {OUTBOX_COLS} FROM outbox WHERE id = ?");
        sqlx::query(sqlx::AssertSqlSafe(sql))
            .bind(id)
            .fetch_optional(&self.pool)
            .await?
            .as_ref()
            .map(outbox_from)
            .transpose()
    }

    async fn outbox_due(
        &self,
        now: DateTime<Utc>,
        limit: usize,
    ) -> StorageResult<Vec<OutboxEntry>> {
        // Как `focus_core::outbox::select_due`: ждущие и наступившие, но не
        // те, перед которыми у того же плагина есть ещё не наступившая запись.
        let sql = format!(
            "SELECT {OUTBOX_COLS} FROM outbox o
             WHERE o.done_at IS NULL
               AND (o.next_attempt_at IS NULL OR o.next_attempt_at <= ?1)
               AND NOT EXISTS (
                   SELECT 1 FROM outbox p
                   WHERE p.plugin_id = o.plugin_id AND p.done_at IS NULL AND p.id < o.id
                     AND p.next_attempt_at > ?1)
             ORDER BY o.id
             LIMIT ?2"
        );
        let limit = i64::try_from(limit).unwrap_or(i64::MAX);
        sqlx::query(sqlx::AssertSqlSafe(sql))
            .bind(ts(now))
            .bind(limit)
            .fetch_all(&self.pool)
            .await?
            .iter()
            .map(outbox_from)
            .collect()
    }

    async fn outbox_done(&self, id: OutboxId, now: DateTime<Utc>) -> StorageResult<bool> {
        let r = sqlx::query("UPDATE outbox SET done_at = ?, next_attempt_at = NULL WHERE id = ?")
            .bind(ts(now))
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(r.rows_affected() == 1)
    }

    async fn outbox_failed(
        &self,
        id: OutboxId,
        error: &str,
        now: DateTime<Utc>,
    ) -> StorageResult<Option<DateTime<Utc>>> {
        let Some(e) = self.outbox_entry(id).await? else {
            return Ok(None);
        };
        let next = e.retry_at(now);
        sqlx::query(
            "UPDATE outbox SET attempts = attempts + 1, last_error = ?, last_error_at = ?,
                               next_attempt_at = ?
             WHERE id = ?",
        )
        .bind(error)
        .bind(ts(now))
        .bind(ts(next))
        .bind(id)
        .execute(&self.pool)
        .await?;
        Ok(Some(next))
    }

    async fn outbox_stats(&self) -> StorageResult<Vec<OutboxStats>> {
        let rows = sqlx::query(
            "SELECT plugin_id,
                    SUM(done_at IS NULL) AS pending,
                    (SELECT e.last_error FROM outbox e
                     WHERE e.plugin_id = o.plugin_id AND e.done_at IS NULL
                       AND e.last_error_at IS NOT NULL
                     ORDER BY e.last_error_at DESC, e.id DESC LIMIT 1) AS last_error,
                    (SELECT MAX(e.last_error_at) FROM outbox e
                     WHERE e.plugin_id = o.plugin_id AND e.done_at IS NULL) AS last_error_at
             FROM outbox o
             GROUP BY plugin_id
             ORDER BY plugin_id",
        )
        .fetch_all(&self.pool)
        .await?;
        rows.iter()
            .map(|r| {
                let pending = r.int("pending")?;
                Ok(OutboxStats {
                    plugin_id: r.text("plugin_id")?,
                    pending: u64::try_from(pending).unwrap_or(0),
                    last_error: r.text_opt("last_error")?,
                    last_error_at: r.ts_opt("last_error_at")?,
                })
            })
            .collect()
    }
}

fn no_entry(id: OutboxId) -> CoreError {
    CoreError::storage(format!("нет записи outbox {id}"))
}

#[async_trait]
impl EventSink for Storage {
    async fn publish(&self, events: &[DomainEvent]) -> CoreResult<()> {
        Ok(self.publish_events(events).await?)
    }
}

#[async_trait]
impl Outbox for Storage {
    async fn set_subscribers(&self, plugin_ids: &[String]) -> CoreResult<()> {
        self.set_outbox_subscribers(plugin_ids);
        Ok(())
    }

    async fn fetch_due(&self, now: DateTime<Utc>, limit: usize) -> CoreResult<Vec<OutboxEntry>> {
        Ok(self.outbox_due(now, limit).await?)
    }

    async fn mark_done(&self, id: OutboxId, now: DateTime<Utc>) -> CoreResult<()> {
        if self.outbox_done(id, now).await? {
            Ok(())
        } else {
            Err(no_entry(id))
        }
    }

    async fn mark_failed(
        &self,
        id: OutboxId,
        error: &str,
        now: DateTime<Utc>,
    ) -> CoreResult<DateTime<Utc>> {
        self.outbox_failed(id, error, now)
            .await?
            .ok_or_else(|| no_entry(id))
    }

    async fn stats(&self) -> CoreResult<Vec<OutboxStats>> {
        Ok(self.outbox_stats().await?)
    }
}
