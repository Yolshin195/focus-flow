//! Настройки и состояние плагинов.

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Настройки сервиса: `GET`/`PUT /api/settings`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SettingsDto {
    /// Длина работы, минут (по умолчанию 50).
    pub work_min: u32,
    /// Длина отдыха, минут (по умолчанию 10).
    pub rest_min: u32,
    /// Норма дня, минут (по умолчанию 480).
    pub daily_target_min: u32,
    /// Настройки плагинов: id плагина → его JSON (схему знает плагин).
    /// Секреты (токены) сюда не возвращаются.
    #[serde(default)]
    pub plugins: BTreeMap<String, serde_json::Value>,
}

impl Default for SettingsDto {
    fn default() -> Self {
        Self {
            work_min: 50,
            rest_min: 10,
            daily_target_min: 8 * 60,
            plugins: BTreeMap::new(),
        }
    }
}

/// Состояние одного плагина: элемент ответа `GET /api/plugins/status`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginStatusDto {
    /// Id плагина (например, `jira`).
    pub id: String,
    /// Включён ли в конфиге.
    pub enabled: bool,
    /// `true`, если последняя доставка прошла без ошибки.
    pub healthy: bool,
    /// Сколько событий outbox ждут доставки этому плагину.
    pub outbox_pending: u64,
    /// Текст последней ошибки.
    #[serde(default)]
    pub last_error: Option<String>,
    /// Когда была последняя ошибка.
    #[serde(default)]
    pub last_error_at: Option<DateTime<Utc>>,
}
