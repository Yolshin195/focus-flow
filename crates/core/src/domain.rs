//! Сущности ядра: рабочий день, задача (узел дерева), сессия, настройки.
//!
//! Перечисления и id берутся из контракта (`focus-api-types`), чтобы ядро,
//! хранилище и API говорили на одном языке; сами сущности — свои, без полей,
//! которые вычисляются (длительность, `ends_at`, дети).

use std::collections::BTreeMap;

use chrono::{DateTime, Duration, NaiveDate, Utc};
use focus_api_types::{
    DayId, DayInfoDto, EndReason, SessionId, SessionKind, SettingsDto, TaskDto, TaskId, TaskStatus,
};
use serde::{Deserialize, Serialize};

/// Рабочий день (`work_days`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkDay {
    /// Id дня.
    pub id: DayId,
    /// Календарная дата в локальном поясе.
    pub date: NaiveDate,
    /// Момент открытия (вручную или стартом первой сессии).
    pub opened_at: DateTime<Utc>,
    /// Момент закрытия; `None`, пока день открыт.
    pub closed_at: Option<DateTime<Utc>>,
    /// Норма дня в секундах, зафиксированная при открытии.
    pub target_sec: i64,
}

impl WorkDay {
    /// Открыт ли день.
    pub fn is_open(&self) -> bool {
        self.closed_at.is_none()
    }

    /// Шапка дня для API.
    pub fn to_dto(&self) -> DayInfoDto {
        DayInfoDto {
            id: self.id,
            date: self.date,
            opened_at: self.opened_at,
            closed_at: self.closed_at,
            target_sec: self.target_sec,
        }
    }
}

/// Узел дерева задач (`tasks`): задача верхнего уровня или пункт.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Task {
    /// Id.
    pub id: TaskId,
    /// Родитель; `None` — узел верхнего уровня.
    pub parent_id: Option<TaskId>,
    /// Название (не пустое).
    pub title: String,
    /// Заметки.
    pub notes: Option<String>,
    /// Статус.
    pub status: TaskStatus,
    /// На какой день запланирована.
    pub planned_for: Option<NaiveDate>,
    /// Оценка в интервалах.
    pub estimate_intervals: Option<u32>,
    /// Порядок среди соседей.
    pub sort_order: i64,
    /// Создана.
    pub created_at: DateTime<Utc>,
    /// Когда стала `done`.
    pub done_at: Option<DateTime<Utc>>,
    /// Ссылка на внешний источник (задача Jira), если есть.
    pub external_url: Option<String>,
}

impl Task {
    /// DTO без детей.
    pub fn to_dto(&self) -> TaskDto {
        TaskDto {
            id: self.id,
            parent_id: self.parent_id,
            title: self.title.clone(),
            notes: self.notes.clone(),
            status: self.status,
            planned_for: self.planned_for,
            estimate_intervals: self.estimate_intervals,
            sort_order: self.sort_order,
            created_at: self.created_at,
            done_at: self.done_at,
            external_url: self.external_url.clone(),
            children: Vec::new(),
        }
    }

    /// Меняет статус, поддерживая `done_at`. Возвращает `true`, если узел
    /// только что стал `done`.
    pub fn set_status(&mut self, status: TaskStatus, now: DateTime<Utc>) -> bool {
        let became_done = status == TaskStatus::Done && self.status != TaskStatus::Done;
        self.status = status;
        if status == TaskStatus::Done {
            if self.done_at.is_none() {
                self.done_at = Some(now);
            }
        } else {
            self.done_at = None;
        }
        became_done
    }
}

/// Сессия (`sessions`). Хранится только `started_at` (+ лимит), всё
/// остальное вычисляется.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Session {
    /// Id.
    pub id: SessionId,
    /// День старта (сессия через полночь остаётся в нём).
    pub day_id: DayId,
    /// Вид.
    pub kind: SessionKind,
    /// Пункт — только у `work`.
    pub task_id: Option<TaskId>,
    /// Начало.
    pub started_at: DateTime<Utc>,
    /// Конец; `None` — активна.
    pub ended_at: Option<DateTime<Utc>>,
    /// Лимит в секундах, зафиксированный при старте; `None` у планирования.
    pub limit_sec: Option<i64>,
    /// Причина завершения.
    pub end_reason: Option<EndReason>,
}

impl Session {
    /// Активна ли сессия.
    pub fn is_active(&self) -> bool {
        self.ended_at.is_none()
    }

    /// Плановый конец: `started_at + limit_sec`; у планирования — `None`.
    pub fn ends_at(&self) -> Option<DateTime<Utc>> {
        self.limit_sec
            .map(|l| self.started_at + Duration::seconds(l))
    }

    /// Фактический конец на момент `now` (у активной — `now`).
    pub fn end_or(&self, now: DateTime<Utc>) -> DateTime<Utc> {
        self.ended_at.unwrap_or(now).max(self.started_at)
    }

    /// Длительность в секундах на момент `now`.
    pub fn duration_sec(&self, now: DateTime<Utc>) -> i64 {
        (self.end_or(now) - self.started_at).num_seconds()
    }

    /// Идёт ли в норму (`work` и `planning`).
    pub fn counts_to_norm(&self) -> bool {
        matches!(self.kind, SessionKind::Work | SessionKind::Planning)
    }
}

/// Настройки сервиса (`settings`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Settings {
    /// Длина работы, минут.
    pub work_min: u32,
    /// Длина отдыха, минут.
    pub rest_min: u32,
    /// Норма дня, минут.
    pub daily_target_min: u32,
    /// Настройки плагинов (схему знает плагин).
    pub plugins: BTreeMap<String, serde_json::Value>,
}

impl Settings {
    /// Лимит работы в секундах.
    pub fn work_sec(&self) -> i64 {
        i64::from(self.work_min) * 60
    }

    /// Длина отдыха в секундах.
    pub fn rest_sec(&self) -> i64 {
        i64::from(self.rest_min) * 60
    }

    /// Норма дня в секундах.
    pub fn target_sec(&self) -> i64 {
        i64::from(self.daily_target_min) * 60
    }

    /// Лимит сессии данного вида при старте.
    pub fn limit_for(&self, kind: SessionKind) -> Option<i64> {
        match kind {
            SessionKind::Work => Some(self.work_sec()),
            SessionKind::Rest => Some(self.rest_sec()),
            SessionKind::Planning => None,
        }
    }
}

impl Default for Settings {
    fn default() -> Self {
        SettingsDto::default().into()
    }
}

impl From<SettingsDto> for Settings {
    fn from(s: SettingsDto) -> Self {
        Self {
            work_min: s.work_min,
            rest_min: s.rest_min,
            daily_target_min: s.daily_target_min,
            plugins: s.plugins,
        }
    }
}

impl From<Settings> for SettingsDto {
    fn from(s: Settings) -> Self {
        Self {
            work_min: s.work_min,
            rest_min: s.rest_min,
            daily_target_min: s.daily_target_min,
            plugins: s.plugins,
        }
    }
}

/// Пункт импортируемой задачи (строка чек-листа).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImportedItem {
    /// Текст пункта.
    pub title: String,
    /// Статус в источнике.
    pub status: TaskStatus,
}

/// Задача, полученная из внешнего источника (`TaskSource`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImportedTask {
    /// Id плагина-источника (например, `jira`).
    pub source: String,
    /// Каноническая ссылка: по ней повторный импорт находит уже созданную задачу.
    pub url: String,
    /// Название задачи.
    pub title: String,
    /// Заметки (описание).
    pub notes: Option<String>,
    /// Пункты в порядке источника.
    pub items: Vec<ImportedItem>,
}
