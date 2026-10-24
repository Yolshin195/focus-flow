//! Настройки плагина (`SettingsDto.plugins["jira"]`) и подключение к Jira.
//! Токен — не здесь, а в секретах плагина (`PluginCtx::secrets`, имя `token`).

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Имя секрета с токеном.
pub const TOKEN_SECRET: &str = "token";

/// Вид Jira и способ авторизации.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    /// Data Center / Server: `Authorization: Bearer <PAT>`.
    #[default]
    DataCenter,
    /// Cloud: Basic `email:api_token`.
    Cloud,
}

/// Какие сессии списывать в worklog.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct WorklogSettings {
    /// Работа, закрытая «Готово», по лимиту или сменой сессии.
    pub work: bool,
    /// Работа, закрытая «Прервать».
    pub interrupted: bool,
    /// Планирование (на задачу пункта предыдущей работы).
    pub planning: bool,
}

impl Default for WorklogSettings {
    fn default() -> Self {
        Self {
            work: true,
            interrupted: true,
            planning: false,
        }
    }
}

/// Настройки плагина Jira.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct JiraSettings {
    /// Флаг включения (читает сервер); без него плагин включён.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    /// Адрес Jira, например `https://jira.example.com` (без `/` в конце).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    /// Data Center или Cloud.
    pub mode: Mode,
    /// Email учётной записи (только Cloud).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    /// Id текстового поля чек-листа «Checklists» (`customfield_10100`); без него — автопоиск.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub checklist_field_id: Option<String>,
    /// Правила worklog.
    pub worklog: WorklogSettings,
}

impl JiraSettings {
    /// Из JSON настроек; `Null` — умолчания.
    pub fn from_value(v: &Value) -> Result<Self, String> {
        if v.is_null() {
            return Ok(Self::default());
        }
        serde_json::from_value(v.clone()).map_err(|e| format!("настройки Jira: {e}"))
    }

    /// `base_url` без пробелов и `/` в конце; пустой — `None`.
    pub fn base(&self) -> Option<String> {
        self.base_url
            .as_deref()
            .map(|s| s.trim().trim_end_matches('/').to_owned())
            .filter(|s| !s.is_empty())
    }

    /// Хост `base_url` в нижнем регистре.
    pub fn host(&self) -> Option<String> {
        let base = self.base()?;
        let u = reqwest::Url::parse(&base).ok()?;
        u.host_str().map(str::to_lowercase)
    }

    /// Подключение, если хватает настроек и токена; иначе — чего не хватает.
    pub fn conn(&self, token: Option<String>) -> Result<Conn, String> {
        let base = self.base().ok_or("не задан base_url")?;
        let token = token.filter(|t| !t.trim().is_empty()).ok_or("нет токена")?;
        let email = match self.mode {
            Mode::Cloud => Some(
                self.email
                    .clone()
                    .filter(|e| !e.trim().is_empty())
                    .ok_or("для cloud нужен email")?,
            ),
            Mode::DataCenter => None,
        };
        Ok(Conn {
            base,
            mode: self.mode,
            email,
            token,
        })
    }
}

/// Проверка `base_url`: `http(s)://хост[/путь]`.
pub fn validate_base_url(s: &str) -> Result<String, String> {
    let s = s.trim().trim_end_matches('/');
    if !s.contains("://") {
        return Err(format!(
            "base_url «{s}»: нужен адрес со схемой, например https://{s}"
        ));
    }
    let u = reqwest::Url::parse(s).map_err(|e| format!("base_url «{s}»: {e}"))?;
    if !matches!(u.scheme(), "http" | "https") || u.host_str().is_none() {
        return Err(format!("base_url «{s}»: нужен http(s)://хост"));
    }
    if u.query().is_some() || u.fragment().is_some() {
        return Err(format!("base_url «{s}»: без ? и #"));
    }
    Ok(s.to_owned())
}

/// Всё, что нужно для запросов к Jira.
#[derive(Clone)]
pub struct Conn {
    /// Адрес без `/` в конце.
    pub base: String,
    /// Режим авторизации.
    pub mode: Mode,
    /// Email (Cloud).
    pub email: Option<String>,
    /// PAT или API-токен.
    pub token: String,
}

impl std::fmt::Debug for Conn {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Токен в логи не попадает.
        f.debug_struct("Conn")
            .field("base", &self.base)
            .field("mode", &self.mode)
            .field("email", &self.email)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn defaults_and_conn() {
        let s = JiraSettings::from_value(&Value::Null).unwrap();
        assert_eq!(s.mode, Mode::DataCenter);
        assert!(s.worklog.work && s.worklog.interrupted && !s.worklog.planning);
        assert_eq!(s.conn(Some("t".into())).unwrap_err(), "не задан base_url");

        let s = JiraSettings::from_value(&json!({
            "base_url": " https://jira.example.com/ ", "mode": "cloud"
        }))
        .unwrap();
        assert_eq!(s.base().as_deref(), Some("https://jira.example.com"));
        assert_eq!(s.host().as_deref(), Some("jira.example.com"));
        assert_eq!(s.conn(None).unwrap_err(), "нет токена");
        assert_eq!(
            s.conn(Some("t".into())).unwrap_err(),
            "для cloud нужен email"
        );
        let c = JiraSettings {
            email: Some("a@b".into()),
            ..s
        }
        .conn(Some("секрет".into()))
        .unwrap();
        assert!(!format!("{c:?}").contains("секрет"));
        assert!(JiraSettings::from_value(&json!({"mode": "x"})).is_err());
    }

    #[test]
    fn base_url_validation() {
        assert_eq!(
            validate_base_url("https://j.example.com/jira/").unwrap(),
            "https://j.example.com/jira"
        );
        assert!(validate_base_url("ftp://x").is_err());
        assert!(validate_base_url("jira.example.com").is_err());
        assert!(validate_base_url("https://x/?a=1").is_err());
    }
}
