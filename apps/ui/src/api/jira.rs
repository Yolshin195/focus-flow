//! Настройки плагина Jira: `GET`/`PUT /api/plugins/jira/settings`,
//! `POST /api/plugins/jira/test`.
//!
//! Это маршруты плагина, а не ядра, поэтому они не входят в `FocusApi`
//! (контракт `api-types`): отдельный трейт [`JiraSettingsApi`] с реализациями
//! для [`HttpApi`](super::HttpApi) и мока [`MockJira`]. Типы ответа — свои,
//! по JSON плагина; неизвестные поля пропускаются, новые — с `default`.

use std::cell::RefCell;

use async_trait::async_trait;
use focus_api_types::{ApiError, ApiResult};
use serde::{Deserialize, Serialize};

/// Режим авторизации Jira.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JiraMode {
    /// Data Center / Server: персональный токен (PAT), `Bearer`.
    #[default]
    DataCenter,
    /// Cloud: email + API-токен, Basic.
    Cloud,
}

/// Что списывать в worklog.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct JiraWorklog {
    /// Работа (кроме прерванной).
    pub work: bool,
    /// Прерванная работа.
    pub interrupted: bool,
    /// Планирование.
    pub planning: bool,
}

impl Default for JiraWorklog {
    fn default() -> Self {
        Self {
            work: true,
            interrupted: true,
            planning: false,
        }
    }
}

/// Настройки и состояние плагина: ответ `GET /api/plugins/jira/settings`.
/// Токена здесь нет — только `has_token`.
#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize)]
#[serde(default)]
pub struct JiraState {
    /// Плагин включён.
    pub enabled: bool,
    /// Всё нужное задано (адрес, токен, email для Cloud).
    pub configured: bool,
    /// Чего не хватает, если не настроен.
    pub problem: Option<String>,
    /// Адрес Jira.
    pub base_url: Option<String>,
    /// Режим.
    pub mode: JiraMode,
    /// Email (Cloud).
    pub email: Option<String>,
    /// Id поля чек-листа, заданный вручную.
    pub checklist_field_id: Option<String>,
    /// Id поля, которым плагин пользуется (заданный или найденный).
    pub checklist_field_resolved: Option<String>,
    /// Что списывать.
    pub worklog: JiraWorklog,
    /// Токен сохранён.
    pub has_token: bool,
    /// Последняя ошибка обращения к Jira.
    pub last_error: Option<String>,
}

/// Правка настроек (`PUT`): переданные поля заменяются, пустая строка
/// очищает. `token: None` — не трогать сохранённый токен.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize)]
pub struct JiraPatch {
    /// Адрес Jira (`""` — очистить).
    pub base_url: String,
    /// Режим.
    pub mode: JiraMode,
    /// Email (`""` — очистить).
    pub email: String,
    /// Id поля чек-листа (`""` — искать автоматически).
    pub checklist_field_id: String,
    /// Что списывать.
    pub worklog: JiraWorklog,
    /// Новый токен; не передаётся, если `None`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
}

/// Ответ `POST /api/plugins/jira/test`.
#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize)]
#[serde(default)]
pub struct JiraTestResult {
    /// Подключение работает.
    pub ok: bool,
    /// Имя пользователя токена.
    pub user: String,
    /// Адрес, к которому подключались.
    pub base_url: String,
}

/// Настройки плагина Jira.
#[async_trait(?Send)]
pub trait JiraSettingsApi {
    /// Текущие настройки и состояние.
    async fn jira_settings(&self) -> ApiResult<JiraState>;
    /// Сохранить настройки; возвращает новое состояние.
    async fn save_jira_settings(&self, patch: JiraPatch) -> ApiResult<JiraState>;
    /// Проверить подключение (`GET /rest/api/2/myself` в Jira).
    async fn test_jira(&self) -> ApiResult<JiraTestResult>;
}

/// Мок настроек Jira для демо и тестов: всё в памяти, проверка «подключается»,
/// если плагин настроен.
#[derive(Debug, Default)]
pub struct MockJira {
    state: RefCell<JiraState>,
}

impl MockJira {
    /// Ненастроенный плагин (как после установки).
    pub fn unconfigured() -> Self {
        let m = Self::default();
        m.recompute();
        m
    }

    /// Настроенный демо-плагин (`https://jira.example.com`, токен сохранён).
    pub fn demo() -> Self {
        let m = Self {
            state: RefCell::new(JiraState {
                base_url: Some("https://jira.example.com".into()),
                has_token: true,
                checklist_field_resolved: Some("customfield_10100".into()),
                ..JiraState::default()
            }),
        };
        m.recompute();
        m
    }

    fn recompute(&self) {
        let mut s = self.state.borrow_mut();
        s.enabled = true;
        s.problem = if s.base_url.is_none() {
            Some("не задан base_url".into())
        } else if !s.has_token {
            Some("не задан токен".into())
        } else if s.mode == JiraMode::Cloud && s.email.is_none() {
            Some("для Cloud нужен email".into())
        } else {
            None
        };
        s.configured = s.problem.is_none();
    }
}

fn non_empty(s: &str) -> Option<String> {
    let s = s.trim();
    (!s.is_empty()).then(|| s.to_owned())
}

#[async_trait(?Send)]
impl JiraSettingsApi for MockJira {
    async fn jira_settings(&self) -> ApiResult<JiraState> {
        Ok(self.state.borrow().clone())
    }

    async fn save_jira_settings(&self, patch: JiraPatch) -> ApiResult<JiraState> {
        let base = non_empty(&patch.base_url);
        if let Some(b) = &base {
            if !(b.starts_with("http://") || b.starts_with("https://")) {
                return Err(ApiError::Validation(
                    "base_url: нужен адрес вида https://jira.example.com".into(),
                ));
            }
        }
        {
            let mut s = self.state.borrow_mut();
            s.base_url = base.map(|b| b.trim_end_matches('/').to_owned());
            s.mode = patch.mode;
            s.email = non_empty(&patch.email);
            s.checklist_field_id = non_empty(&patch.checklist_field_id);
            s.checklist_field_resolved = s
                .checklist_field_id
                .clone()
                .or_else(|| s.base_url.as_ref().map(|_| "customfield_10100".into()));
            s.worklog = patch.worklog;
            if let Some(t) = patch.token {
                s.has_token = !t.trim().is_empty();
            }
        }
        self.recompute();
        Ok(self.state.borrow().clone())
    }

    async fn test_jira(&self) -> ApiResult<JiraTestResult> {
        let s = self.state.borrow();
        if let Some(why) = &s.problem {
            return Err(ApiError::Validation(format!("Jira не настроена: {why}")));
        }
        Ok(JiraTestResult {
            ok: true,
            user: "Демо Пользователь".into(),
            base_url: s.base_url.clone().unwrap_or_default(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn block_on<F: std::future::Future>(f: F) -> F::Output {
        use std::pin::pin;
        use std::task::{Context, Poll, Waker};
        let mut f = pin!(f);
        let mut cx = Context::from_waker(Waker::noop());
        loop {
            if let Poll::Ready(v) = f.as_mut().poll(&mut cx) {
                return v;
            }
        }
    }

    #[test]
    fn state_parses_plugin_json() {
        let json = r#"{"enabled":true,"configured":false,"problem":"не задан base_url",
            "base_url":null,"mode":"cloud","email":"me@example.com","checklist_field_id":null,
            "checklist_field_resolved":null,"worklog":{"work":true,"interrupted":false,"planning":true},
            "has_token":true,"last_error":null,"future_field":1}"#;
        let s: JiraState = serde_json::from_str(json).unwrap();
        assert_eq!(s.mode, JiraMode::Cloud);
        assert!(s.has_token && !s.configured);
        assert_eq!(
            s.worklog,
            JiraWorklog {
                work: true,
                interrupted: false,
                planning: true
            }
        );
    }

    #[test]
    fn patch_skips_token_when_unchanged() {
        let p = JiraPatch::default();
        let v = serde_json::to_value(&p).unwrap();
        assert!(v.get("token").is_none());
        assert_eq!(v["mode"], "data_center");
        assert_eq!(v["base_url"], "");
    }

    #[test]
    fn mock_requires_email_for_cloud() {
        let m = MockJira::unconfigured();
        let s = block_on(m.jira_settings()).unwrap();
        assert!(!s.configured);
        let s = block_on(m.save_jira_settings(JiraPatch {
            base_url: "https://x.atlassian.net/".into(),
            mode: JiraMode::Cloud,
            token: Some("t".into()),
            ..JiraPatch::default()
        }))
        .unwrap();
        assert_eq!(s.base_url.as_deref(), Some("https://x.atlassian.net"));
        assert!(!s.configured);
        assert!(block_on(m.test_jira()).is_err());
        let s = block_on(m.save_jira_settings(JiraPatch {
            base_url: "https://x.atlassian.net".into(),
            mode: JiraMode::Cloud,
            email: "me@x".into(),
            ..JiraPatch::default()
        }))
        .unwrap();
        assert!(s.configured && s.has_token);
        assert!(block_on(m.test_jira()).unwrap().ok);
    }
}
