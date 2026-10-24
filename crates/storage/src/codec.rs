//! Преобразование значений домена в колонки SQLite и обратно.
//!
//! - UUID — TEXT (`Uuid::to_string`, нижний регистр с дефисами);
//! - моменты — TEXT RFC3339 UTC с наносекундами фиксированной длины, поэтому
//!   сравнение строк в SQL совпадает со сравнением моментов;
//! - даты — TEXT `YYYY-MM-DD`;
//! - перечисления — их snake_case-имя из serde (как в JSON контракта).

use std::str::FromStr;

use chrono::{DateTime, NaiveDate, SecondsFormat, Utc};
use serde::Serialize;
use serde::de::DeserializeOwned;
use sqlx::Row;
use sqlx::sqlite::SqliteRow;

use crate::error::{StorageError, StorageResult};

/// Момент → TEXT.
pub fn ts(at: DateTime<Utc>) -> String {
    at.to_rfc3339_opts(SecondsFormat::Nanos, true)
}

/// Необязательный момент → TEXT/NULL.
pub fn ts_opt(at: Option<DateTime<Utc>>) -> Option<String> {
    at.map(ts)
}

/// Дата → TEXT.
pub fn date(d: NaiveDate) -> String {
    d.format("%Y-%m-%d").to_string()
}

/// Перечисление → его serde-имя.
pub fn enum_str<T: Serialize>(v: &T) -> StorageResult<String> {
    match serde_json::to_value(v)? {
        serde_json::Value::String(s) => Ok(s),
        other => Err(StorageError::Decode(format!(
            "перечисление сериализовалось не в строку: {other}"
        ))),
    }
}

fn parse_enum<T: DeserializeOwned>(col: &str, s: String) -> StorageResult<T> {
    serde_json::from_value(serde_json::Value::String(s))
        .map_err(|e| StorageError::Decode(format!("{col}: {e}")))
}

fn parse_ts(col: &str, s: &str) -> StorageResult<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s)
        .map(|d| d.with_timezone(&Utc))
        .map_err(|e| StorageError::Decode(format!("{col}: «{s}»: {e}")))
}

/// Чтение типизированных колонок строки результата.
pub trait RowExt {
    /// TEXT NOT NULL.
    fn text(&self, col: &str) -> StorageResult<String>;
    /// TEXT NULL.
    fn text_opt(&self, col: &str) -> StorageResult<Option<String>>;
    /// INTEGER NOT NULL.
    fn int(&self, col: &str) -> StorageResult<i64>;
    /// INTEGER NULL.
    fn int_opt(&self, col: &str) -> StorageResult<Option<i64>>;

    /// Id (`FromStr` поверх UUID).
    fn id<T: FromStr>(&self, col: &str) -> StorageResult<T>
    where
        T::Err: std::fmt::Display,
    {
        let s = self.text(col)?;
        s.parse()
            .map_err(|e| StorageError::Decode(format!("{col}: «{s}»: {e}")))
    }

    /// Необязательный id.
    fn id_opt<T: FromStr>(&self, col: &str) -> StorageResult<Option<T>>
    where
        T::Err: std::fmt::Display,
    {
        self.text_opt(col)?
            .map(|s| {
                s.parse()
                    .map_err(|e| StorageError::Decode(format!("{col}: «{s}»: {e}")))
            })
            .transpose()
    }

    /// Момент.
    fn ts(&self, col: &str) -> StorageResult<DateTime<Utc>> {
        parse_ts(col, &self.text(col)?)
    }

    /// Необязательный момент.
    fn ts_opt(&self, col: &str) -> StorageResult<Option<DateTime<Utc>>> {
        self.text_opt(col)?.map(|s| parse_ts(col, &s)).transpose()
    }

    /// Дата.
    fn date(&self, col: &str) -> StorageResult<NaiveDate> {
        let s = self.text(col)?;
        NaiveDate::parse_from_str(&s, "%Y-%m-%d")
            .map_err(|e| StorageError::Decode(format!("{col}: «{s}»: {e}")))
    }

    /// Необязательная дата.
    fn date_opt(&self, col: &str) -> StorageResult<Option<NaiveDate>> {
        self.text_opt(col)?
            .map(|s| {
                NaiveDate::parse_from_str(&s, "%Y-%m-%d")
                    .map_err(|e| StorageError::Decode(format!("{col}: «{s}»: {e}")))
            })
            .transpose()
    }

    /// Перечисление по serde-имени.
    fn enum_of<T: DeserializeOwned>(&self, col: &str) -> StorageResult<T> {
        parse_enum(col, self.text(col)?)
    }

    /// Необязательное перечисление.
    fn enum_opt<T: DeserializeOwned>(&self, col: &str) -> StorageResult<Option<T>> {
        self.text_opt(col)?.map(|s| parse_enum(col, s)).transpose()
    }
}

impl RowExt for SqliteRow {
    fn text(&self, col: &str) -> StorageResult<String> {
        Ok(self.try_get(col)?)
    }

    fn text_opt(&self, col: &str) -> StorageResult<Option<String>> {
        Ok(self.try_get(col)?)
    }

    fn int(&self, col: &str) -> StorageResult<i64> {
        Ok(self.try_get(col)?)
    }

    fn int_opt(&self, col: &str) -> StorageResult<Option<i64>> {
        Ok(self.try_get(col)?)
    }
}

#[cfg(test)]
mod tests {
    use chrono::{Duration, TimeZone};

    use super::*;

    #[test]
    fn timestamps_sort_as_strings() {
        let a = Utc.with_ymd_and_hms(2026, 10, 24, 9, 0, 0).unwrap();
        let b = a + Duration::nanoseconds(1);
        let c = a + Duration::hours(30);
        assert_eq!(ts(a), "2026-10-24T09:00:00.000000000Z");
        assert!(ts(a) < ts(b) && ts(b) < ts(c));
        assert_eq!(parse_ts("x", &ts(b)).unwrap(), b);
    }

    #[test]
    fn enums_use_serde_names() {
        use focus_api_types::{EndReason, TaskStatus};
        assert_eq!(enum_str(&TaskStatus::InProgress).unwrap(), "in_progress");
        let r: EndReason = parse_enum("x", "task_done".into()).unwrap();
        assert_eq!(r, EndReason::TaskDone);
        assert!(parse_enum::<EndReason>("x", "nope".into()).is_err());
    }
}
