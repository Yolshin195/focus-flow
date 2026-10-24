//! Часы: «сейчас» и границы локального дня.

use std::sync::Mutex;

use chrono::{DateTime, Duration, FixedOffset, Local, NaiveDate, Offset, Utc};

/// Порт часов. Время в домене — UTC; календарная дата — в локальном поясе.
pub trait Clock: Send + Sync {
    /// Текущий момент.
    fn now(&self) -> DateTime<Utc>;

    /// Локальная календарная дата момента `at` (к ней относится день).
    fn local_date(&self, at: DateTime<Utc>) -> NaiveDate;
}

/// Системные часы: `Utc::now()` и локальный пояс системы (`chrono::Local`).
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> DateTime<Utc> {
        Utc::now()
    }

    fn local_date(&self, at: DateTime<Utc>) -> NaiveDate {
        at.with_timezone(&Local).date_naive()
    }
}

/// Управляемые часы с фиксированным смещением пояса — для тестов и мока.
#[derive(Debug)]
pub struct ManualClock {
    now: Mutex<DateTime<Utc>>,
    offset: FixedOffset,
}

impl ManualClock {
    /// Часы, стоящие в `start`, пояс UTC.
    pub fn new(start: DateTime<Utc>) -> Self {
        Self::with_offset(start, Utc.fix())
    }

    /// Часы, стоящие в `start`, с поясом `offset`.
    pub fn with_offset(start: DateTime<Utc>, offset: FixedOffset) -> Self {
        Self {
            now: Mutex::new(start),
            offset,
        }
    }

    /// Поставить время.
    pub fn set(&self, at: DateTime<Utc>) {
        *self.lock() = at;
    }

    /// Сдвинуть время вперёд (или назад при отрицательном `by`).
    pub fn advance(&self, by: Duration) {
        let mut now = self.lock();
        *now += by;
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, DateTime<Utc>> {
        // Отравление невозможно по смыслу (внутри нет паник), но не паникуем.
        self.now.lock().unwrap_or_else(|e| e.into_inner())
    }
}

impl Clock for ManualClock {
    fn now(&self) -> DateTime<Utc> {
        *self.lock()
    }

    fn local_date(&self, at: DateTime<Utc>) -> NaiveDate {
        at.with_timezone(&self.offset).date_naive()
    }
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    use super::*;

    #[test]
    fn manual_clock_local_date_uses_offset() {
        let at = Utc.with_ymd_and_hms(2026, 10, 24, 20, 0, 0).unwrap();
        let bangkok = FixedOffset::east_opt(7 * 3600).unwrap();
        let clock = ManualClock::with_offset(at, bangkok);
        assert_eq!(
            clock.local_date(clock.now()),
            NaiveDate::from_ymd_opt(2026, 10, 25).unwrap()
        );
        clock.advance(Duration::hours(-4));
        assert_eq!(
            clock.local_date(clock.now()),
            NaiveDate::from_ymd_opt(2026, 10, 24).unwrap()
        );
    }
}
