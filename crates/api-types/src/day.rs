//! Рабочий день, статус «сейчас» и итоги дня.

use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};

use crate::ids::DayId;
use crate::session::{ActiveSession, SessionDto};

/// Шапка рабочего дня (без сессий).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DayInfoDto {
    /// Id дня.
    pub id: DayId,
    /// Календарная дата дня в локальном поясе сервиса.
    pub date: NaiveDate,
    /// Момент открытия (вручную или стартом первой сессии).
    pub opened_at: DateTime<Utc>,
    /// Момент закрытия; `None`, пока день открыт.
    #[serde(default)]
    pub closed_at: Option<DateTime<Utc>>,
    /// Норма дня в секундах (зафиксирована при открытии).
    pub target_sec: i64,
}

/// Текущее состояние: ответ `GET /api/status` и большинства команд.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatusDto {
    /// Время сервиса на момент ответа — клиент считает таймер от него,
    /// а не от своих часов.
    pub server_time: DateTime<Utc>,
    /// Текущий день; `None`, если сегодня день ещё не открывался.
    #[serde(default)]
    pub day: Option<DayInfoDto>,
    /// Активная сессия; `None` — ничего не идёт (время теряется).
    #[serde(default)]
    pub active: Option<ActiveSession>,
    /// Отработано сегодня: `work` + `planning`, в секундах, включая активную сессию.
    pub worked_today_sec: i64,
    /// Норма дня в секундах.
    pub target_sec: i64,
    /// С какого момента идёт потерянное время (конец последней сессии),
    /// если сейчас нет активной сессии и день открыт.
    #[serde(default)]
    pub idle_since: Option<DateTime<Utc>>,
    /// Работа закрыта по лимиту, а отдых ещё не начат: клиент зовёт
    /// пользователя (сигнал до реакции) и предлагает «Начать отдых».
    /// Ожидание — потерянное время.
    #[serde(default)]
    pub rest_due: bool,
}

/// Отрезок времени `[start, end)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TimeRange {
    /// Начало.
    pub start: DateTime<Utc>,
    /// Конец.
    pub end: DateTime<Utc>,
}

/// Итоги дня в секундах.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DayTotalsDto {
    /// Сумма сессий `work`.
    pub work_sec: i64,
    /// Сумма сессий `planning`.
    pub planning_sec: i64,
    /// Сумма сессий `rest`.
    pub rest_sec: i64,
    /// Потерянное время: промежутки без активной сессии внутри открытого дня.
    pub lost_sec: i64,
    /// В норму: `work_sec + planning_sec`.
    pub worked_sec: i64,
    /// Норма дня.
    pub target_sec: i64,
}

/// День для календаря: ответ `GET /api/days/{date}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DayDto {
    /// Запрошенная дата.
    pub date: NaiveDate,
    /// Шапка дня; `None`, если в эту дату день не открывался.
    #[serde(default)]
    pub day: Option<DayInfoDto>,
    /// Сессии дня в порядке начала (включая активную).
    #[serde(default)]
    pub sessions: Vec<SessionDto>,
    /// Промежутки потерянного времени (на календаре — серые).
    #[serde(default)]
    pub lost: Vec<TimeRange>,
    /// Итоги.
    #[serde(default)]
    pub totals: DayTotalsDto,
}
