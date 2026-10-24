//! Записи outbox и политика повторов доставки событий плагинам.
//!
//! Событие, опубликованное ядром, раскладывается на записи — по одной на
//! каждого подписчика (плагин). Воркер (шаг 5) забирает готовые к доставке
//! записи ([`crate::Outbox::fetch_due`]), отмечает доставленные
//! ([`crate::Outbox::mark_done`]) и неудачные ([`crate::Outbox::mark_failed`])
//! с экспоненциальной задержкой [`backoff`].

use chrono::{DateTime, Duration, Utc};

use crate::events::DomainEvent;

/// Первая задержка повтора.
pub const BACKOFF_BASE_SEC: i64 = 10;
/// Предел задержки повтора (1 час).
pub const BACKOFF_MAX_SEC: i64 = 60 * 60;

/// Id записи outbox (возрастает в порядке публикации).
pub type OutboxId = i64;

/// Запись outbox: одно событие для одного плагина.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutboxEntry {
    /// Id записи.
    pub id: OutboxId,
    /// Плагин-получатель.
    pub plugin_id: String,
    /// Событие.
    pub event: DomainEvent,
    /// Сколько неудачных попыток уже было.
    pub attempts: u32,
    /// Не раньше какого момента пробовать снова; `None` — сразу.
    pub next_attempt_at: Option<DateTime<Utc>>,
    /// Текст последней ошибки.
    pub last_error: Option<String>,
    /// Когда была последняя ошибка.
    pub last_error_at: Option<DateTime<Utc>>,
    /// Когда доставлена; `None` — ждёт доставки.
    pub done_at: Option<DateTime<Utc>>,
}

impl OutboxEntry {
    /// Ждёт ли запись доставки.
    pub fn is_pending(&self) -> bool {
        self.done_at.is_none()
    }

    /// Пора ли пробовать доставку в момент `now`.
    pub fn is_due(&self, now: DateTime<Utc>) -> bool {
        self.is_pending() && self.next_attempt_at.is_none_or(|at| at <= now)
    }

    /// Момент следующей попытки, если текущая (в `now`) не удалась.
    pub fn retry_at(&self, now: DateTime<Utc>) -> DateTime<Utc> {
        now + backoff(self.attempts.saturating_add(1))
    }
}

/// Сводка очереди одного плагина (для `/api/plugins/status`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutboxStats {
    /// Плагин.
    pub plugin_id: String,
    /// Сколько записей ждут доставки.
    pub pending: u64,
    /// Последняя ошибка среди недоставленных записей.
    pub last_error: Option<String>,
    /// Когда была эта ошибка.
    pub last_error_at: Option<DateTime<Utc>>,
}

/// Задержка перед повтором после `attempts`-й неудачи (1, 2, …):
/// 10 с, 20 с, 40 с, … но не больше часа.
pub fn backoff(attempts: u32) -> Duration {
    let exp = attempts.saturating_sub(1).min(20);
    let sec = BACKOFF_BASE_SEC.saturating_mul(1_i64 << exp);
    Duration::seconds(sec.min(BACKOFF_MAX_SEC))
}

/// Записи, которые воркер может доставлять в момент `now`: ждущие и
/// наступившие, в порядке id. Порядок по плагину сохраняется: записи плагина
/// после ещё не наступившей (ждущей повтора) не возвращаются. Не больше `limit`.
pub fn select_due<'a>(
    entries: impl IntoIterator<Item = &'a OutboxEntry>,
    now: DateTime<Utc>,
    limit: usize,
) -> Vec<OutboxEntry> {
    let mut sorted: Vec<&OutboxEntry> = entries.into_iter().filter(|e| e.is_pending()).collect();
    sorted.sort_by_key(|e| e.id);
    let mut blocked = std::collections::BTreeSet::new();
    let mut out = Vec::new();
    for e in sorted {
        if out.len() >= limit {
            break;
        }
        if blocked.contains(e.plugin_id.as_str()) {
            continue;
        }
        if e.is_due(now) {
            out.push(e.clone());
        } else {
            blocked.insert(e.plugin_id.as_str());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    use super::*;

    #[test]
    fn backoff_grows_and_caps() {
        assert_eq!(backoff(1), Duration::seconds(10));
        assert_eq!(backoff(2), Duration::seconds(20));
        assert_eq!(backoff(3), Duration::seconds(40));
        assert_eq!(backoff(12), Duration::seconds(BACKOFF_MAX_SEC));
        assert_eq!(backoff(u32::MAX), Duration::seconds(BACKOFF_MAX_SEC));
    }

    #[test]
    fn select_due_keeps_order_per_plugin() {
        let now = Utc.with_ymd_and_hms(2026, 10, 24, 9, 0, 0).unwrap();
        let e = |id, plugin: &str, next: Option<i64>| OutboxEntry {
            id,
            plugin_id: plugin.into(),
            event: DomainEvent::Unknown,
            attempts: 0,
            next_attempt_at: next.map(|m| now + Duration::minutes(m)),
            last_error: None,
            last_error_at: None,
            done_at: None,
        };
        let all = [
            e(1, "a", Some(5)), // ждёт повтора — блокирует 3
            e(2, "b", None),
            e(3, "a", None),
            e(4, "b", Some(-1)),
        ];
        let ids: Vec<_> = select_due(&all, now, 10).iter().map(|x| x.id).collect();
        assert_eq!(ids, [2, 4]);
        let ids: Vec<_> = select_due(&all, now, 1).iter().map(|x| x.id).collect();
        assert_eq!(ids, [2]);
    }
}
