//! Настройки плагина Jira: `GET`/`PUT /api/plugins/jira/settings`,
//! `POST /api/plugins/jira/test`; очередь отправки: `GET /queue`,
//! `POST /queue/send`, `DELETE /queue/{id}`; синхронизация из Jira:
//! `POST /sync` (там же, `/api/plugins/jira`).
//!
//! Это маршруты плагина, а не ядра, поэтому они не входят в `FocusApi`
//! (контракт `api-types`): отдельный трейт [`JiraSettingsApi`] с реализациями
//! для [`HttpApi`](super::HttpApi) и мока [`MockJira`]. Типы ответа — свои,
//! по JSON плагина; неизвестные поля пропускаются, новые — с `default`.

use std::cell::RefCell;

use async_trait::async_trait;
use focus_api_types::chrono::{DateTime, Duration, Utc};
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
    /// Отправлять очередь сразу (иначе — кнопкой).
    pub auto_send: bool,
    /// Забирать изменения из Jira сами (иначе — кнопкой).
    pub auto_sync: bool,
    /// Как часто идёт автосинхронизация, мин (0 — сервис не сказал).
    pub sync_every_min: u64,
    /// Итог последней синхронизации (с запуска сервиса).
    pub last_sync: Option<JiraSyncResult>,
    /// Сколько операций ждут отправки.
    pub queue_pending: u64,
    /// Последняя ошибка обращения к Jira.
    pub last_error: Option<String>,
}

/// Итог синхронизации из Jira: ответ `POST /sync`, `last_sync` в настройках.
#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize)]
#[serde(default)]
pub struct JiraSyncResult {
    /// Когда закончилась.
    pub at: Option<DateTime<Utc>>,
    /// Задач проверено.
    pub tasks: u64,
    /// Задач, в которых что-то изменилось.
    pub changed: u64,
    /// Задач с новым названием.
    pub renamed: u64,
    /// Новых пунктов.
    pub added_items: u64,
    /// Пунктов с новым статусом.
    pub updated_items: u64,
    /// Пунктов, изменённых в Jira, но с неотправленными изменениями у нас.
    pub kept_local: u64,
    /// Задач, которые не удалось получить.
    pub failed: u64,
    /// Первая ошибка.
    pub error: Option<String>,
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
    /// Отправлять очередь сразу.
    pub auto_send: bool,
    /// Синхронизировать из Jira сами.
    pub auto_sync: bool,
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

/// Что плагин собирается сделать в Jira.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JiraOpKind {
    /// Добавить строку `- текст` в чек-лист.
    ChecklistAdd,
    /// Отметить строку `+ текст`.
    ChecklistDone,
    /// Снять отметку: `+ текст` → `- текст`.
    ChecklistUndone,
    /// Записать worklog.
    Worklog,
    /// Вид из более новой версии сервиса.
    #[serde(other)]
    Unknown,
}

/// Операция в очереди отправки.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct JiraOp {
    /// Id записи (для удаления).
    pub id: i64,
    /// Что сделать.
    pub kind: JiraOpKind,
    /// Задача Jira.
    pub issue_key: String,
    /// Текст пункта или комментарий worklog.
    pub text: String,
    /// Worklog: начало.
    #[serde(default)]
    pub started_at: Option<DateTime<Utc>>,
    /// Worklog: длительность, с.
    #[serde(default)]
    pub seconds: Option<i64>,
    /// Когда попала в очередь.
    pub created_at: DateTime<Utc>,
    /// Неудачных попыток.
    #[serde(default)]
    pub attempts: i64,
    /// Последняя ошибка отправки.
    #[serde(default)]
    pub last_error: Option<String>,
}

/// Очередь отправки: ответ `GET /queue`, `POST /queue/send`, `DELETE /queue/{id}`.
#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize)]
#[serde(default)]
pub struct JiraQueue {
    /// Автоотправка включена.
    pub auto_send: bool,
    /// Ждущие операции по порядку.
    pub items: Vec<JiraOp>,
    /// Только после отправки: сколько прошло.
    pub sent: Option<u64>,
    /// Только после отправки: на чём остановилась.
    pub error: Option<String>,
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
    /// Очередь отправки.
    async fn jira_queue(&self) -> ApiResult<JiraQueue>;
    /// Отправить очередь сейчас.
    async fn send_jira_queue(&self) -> ApiResult<JiraQueue>;
    /// Не отправлять операцию `id`.
    async fn drop_jira_op(&self, id: i64) -> ApiResult<JiraQueue>;
    /// Забрать изменения из Jira по всем задачам сейчас.
    async fn sync_jira(&self) -> ApiResult<JiraSyncResult>;
}

/// Мок настроек Jira для демо и тестов: всё в памяти, проверка «подключается»,
/// если плагин настроен, отправка очереди всегда проходит, синхронизация
/// находит задачи без изменений (задачи мока `MockApi` — отдельно).
#[derive(Debug, Default)]
pub struct MockJira {
    state: RefCell<JiraState>,
    queue: RefCell<Vec<JiraOp>>,
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
        let now = Utc::now();
        let op = |id, kind, text: &str, ago_min, seconds: Option<i64>| JiraOp {
            id,
            kind,
            issue_key: "FF-12".into(),
            text: text.into(),
            started_at: seconds.map(|s| now - Duration::minutes(ago_min) - Duration::seconds(s)),
            seconds,
            created_at: now - Duration::minutes(ago_min),
            attempts: 0,
            last_error: None,
        };
        let m = Self {
            state: RefCell::new(JiraState {
                base_url: Some("https://jira.example.com".into()),
                has_token: true,
                checklist_field_resolved: Some("customfield_10100".into()),
                sync_every_min: 5,
                ..JiraState::default()
            }),
            queue: RefCell::new(vec![
                op(
                    1,
                    JiraOpKind::ChecklistAdd,
                    "Проверить экспорт отчёта",
                    95,
                    None,
                ),
                op(
                    2,
                    JiraOpKind::Worklog,
                    "Проверить экспорт отчёта",
                    40,
                    Some(50 * 60),
                ),
                op(
                    3,
                    JiraOpKind::ChecklistDone,
                    "Проверить экспорт отчёта",
                    40,
                    None,
                ),
            ]),
        };
        m.recompute();
        m
    }

    fn queue_now(&self) -> JiraQueue {
        JiraQueue {
            auto_send: self.state.borrow().auto_send,
            items: self.queue.borrow().clone(),
            ..JiraQueue::default()
        }
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
        s.queue_pending = self.queue.borrow().len() as u64;
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
            s.auto_send = patch.auto_send;
            s.auto_sync = patch.auto_sync;
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

    async fn jira_queue(&self) -> ApiResult<JiraQueue> {
        Ok(self.queue_now())
    }

    async fn send_jira_queue(&self) -> ApiResult<JiraQueue> {
        if let Some(why) = self.state.borrow().problem.clone() {
            return Err(ApiError::Validation(format!("Jira не настроена: {why}")));
        }
        let sent = self.queue.borrow_mut().drain(..).count() as u64;
        self.recompute();
        Ok(JiraQueue {
            sent: Some(sent),
            ..self.queue_now()
        })
    }

    async fn drop_jira_op(&self, id: i64) -> ApiResult<JiraQueue> {
        self.queue.borrow_mut().retain(|op| op.id != id);
        self.recompute();
        Ok(self.queue_now())
    }

    async fn sync_jira(&self) -> ApiResult<JiraSyncResult> {
        if let Some(why) = self.state.borrow().problem.clone() {
            return Err(ApiError::Validation(format!("Jira не настроена: {why}")));
        }
        let r = JiraSyncResult {
            at: Some(Utc::now()),
            tasks: 2,
            ..JiraSyncResult::default()
        };
        self.state.borrow_mut().last_sync = Some(r.clone());
        Ok(r)
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

    #[test]
    fn queue_parses_plugin_json() {
        let json = r#"{"auto_send":false,"items":[
            {"id":3,"kind":"worklog","ref_id":"s","issue_key":"ABC-1","text":"пункт",
             "started_at":"2026-10-24T09:00:00Z","seconds":1200,"created_at":"2026-10-24T09:20:00Z",
             "attempts":1,"last_error":"Jira отклонила запрос (400)","last_error_at":"2026-10-24T09:21:00Z"},
            {"id":4,"kind":"new_kind","issue_key":"ABC-1","text":"x","created_at":"2026-10-24T09:20:00Z"}
        ],"sent":0,"error":"ABC-1: Jira отклонила запрос (400)"}"#;
        let q: JiraQueue = serde_json::from_str(json).unwrap();
        assert_eq!(q.items.len(), 2);
        assert_eq!(q.items[0].kind, JiraOpKind::Worklog);
        assert_eq!(q.items[0].seconds, Some(1200));
        assert_eq!(q.items[1].kind, JiraOpKind::Unknown);
        assert_eq!(q.sent, Some(0));
    }

    #[test]
    fn sync_parses_plugin_json() {
        let json = r#"{"at":"2026-10-25T09:00:00Z","tasks":3,"changed":1,"renamed":0,
            "added_items":2,"updated_items":1,"kept_local":1,"failed":1,"error":"ABC-9: 404"}"#;
        let r: JiraSyncResult = serde_json::from_str(json).unwrap();
        assert_eq!((r.tasks, r.added_items, r.kept_local), (3, 2, 1));
        assert_eq!(r.error.as_deref(), Some("ABC-9: 404"));
        let s: JiraState = serde_json::from_str(
            r#"{"auto_sync":true,"sync_every_min":5,"last_sync":{"tasks":1}}"#,
        )
        .unwrap();
        assert!(s.auto_sync);
        assert_eq!(s.last_sync.unwrap().tasks, 1);
        let p = JiraPatch {
            auto_sync: true,
            ..JiraPatch::default()
        };
        assert_eq!(serde_json::to_value(&p).unwrap()["auto_sync"], true);
    }

    #[test]
    fn mock_sync() {
        assert!(block_on(MockJira::unconfigured().sync_jira()).is_err());
        let m = MockJira::demo();
        let r = block_on(m.sync_jira()).unwrap();
        assert!(r.at.is_some() && r.error.is_none());
        assert_eq!(block_on(m.jira_settings()).unwrap().last_sync, Some(r));
    }

    #[test]
    fn mock_queue_drop_and_send() {
        let m = MockJira::demo();
        assert_eq!(block_on(m.jira_settings()).unwrap().queue_pending, 3);
        let q = block_on(m.drop_jira_op(2)).unwrap();
        assert_eq!(q.items.iter().map(|o| o.id).collect::<Vec<_>>(), [1, 3]);
        let q = block_on(m.send_jira_queue()).unwrap();
        assert_eq!(q.sent, Some(2));
        assert!(q.items.is_empty());
        assert_eq!(block_on(m.jira_settings()).unwrap().queue_pending, 0);
        assert!(block_on(MockJira::unconfigured().send_jira_queue()).is_err());
    }
}
