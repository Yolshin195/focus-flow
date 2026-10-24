//! HTTP-клиент Jira REST API v2: авторизация (Bearer для Data Center,
//! Basic для Cloud), разбор ошибок, нужные плагину вызовы.

use chrono::{DateTime, Utc};
use reqwest::{Method, RequestBuilder, StatusCode, header};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use crate::settings::{Conn, Mode};

/// Ошибка обращения к Jira. Любая из них — повод для повтора доставки
/// (outbox); текст виден в `/api/plugins/status`.
#[derive(Debug, thiserror::Error)]
pub enum JiraError {
    /// Сеть, DNS, TLS, таймаут.
    #[error("Jira недоступна: {0}")]
    Network(String),
    /// 5xx.
    #[error("Jira ответила {status}: {message}")]
    Server {
        /// HTTP-статус.
        status: u16,
        /// Текст ошибки.
        message: String,
    },
    /// 4xx — обычно ошибка настройки (токен, режим, права, ключ задачи).
    #[error("Jira отклонила запрос ({status}): {message}")]
    Client {
        /// HTTP-статус.
        status: u16,
        /// Текст ошибки с подсказкой.
        message: String,
    },
    /// Ответ не того вида.
    #[error("неожиданный ответ Jira: {0}")]
    Decode(String),
}

impl JiraError {
    /// HTTP-статус ответа, если ответ был.
    pub fn status(&self) -> Option<u16> {
        match self {
            Self::Server { status, .. } | Self::Client { status, .. } => Some(*status),
            _ => None,
        }
    }
}

/// Поле Jira (`GET /rest/api/2/field`).
#[derive(Debug, Clone, Deserialize)]
pub struct Field {
    /// Id (`customfield_10100`).
    pub id: String,
    /// Название.
    #[serde(default)]
    pub name: String,
}

/// Задача Jira: название и текст чек-листа.
#[derive(Debug, Clone)]
pub struct Issue {
    /// Ключ (`PROJ-1`).
    pub key: String,
    /// Название.
    pub summary: String,
    /// Текст поля чек-листа (`""`, если поле пустое или не задано).
    pub checklist: String,
}

/// Клиент Jira (обёртка над общим `reqwest::Client`).
#[derive(Debug, Clone)]
pub struct Jira<'a> {
    http: &'a reqwest::Client,
    conn: &'a Conn,
}

impl<'a> Jira<'a> {
    /// Клиент для подключения `conn`.
    pub fn new(http: &'a reqwest::Client, conn: &'a Conn) -> Self {
        Self { http, conn }
    }

    fn req(&self, method: Method, path: &str) -> RequestBuilder {
        let rb = self
            .http
            .request(method, format!("{}{path}", self.conn.base))
            .header(header::ACCEPT, "application/json");
        match self.conn.mode {
            Mode::DataCenter => rb.bearer_auth(&self.conn.token),
            Mode::Cloud => rb.basic_auth(
                self.conn.email.as_deref().unwrap_or_default(),
                Some(&self.conn.token),
            ),
        }
    }

    async fn send(&self, rb: RequestBuilder) -> Result<Vec<u8>, JiraError> {
        let resp = rb
            .send()
            .await
            .map_err(|e| JiraError::Network(error_chain(&e)))?;
        let status = resp.status();
        let body = resp
            .bytes()
            .await
            .map_err(|e| JiraError::Network(error_chain(&e)))?;
        if status.is_success() {
            return Ok(body.to_vec());
        }
        let detail = jira_messages(&body);
        if status.is_server_error() {
            return Err(JiraError::Server {
                status: status.as_u16(),
                message: detail.unwrap_or_else(|| reason(status)),
            });
        }
        let hint = match status {
            StatusCode::UNAUTHORIZED => {
                "неверный токен или режим авторизации (data_center — Bearer PAT, cloud — email + API-токен)"
                    .to_owned()
            }
            StatusCode::FORBIDDEN => "нет прав на это действие".to_owned(),
            StatusCode::NOT_FOUND => "не найдено (ключ задачи, base_url или поле)".to_owned(),
            s => reason(s),
        };
        Err(JiraError::Client {
            status: status.as_u16(),
            message: match detail {
                Some(d) => format!("{hint}; {d}"),
                None => hint,
            },
        })
    }

    async fn json<T: DeserializeOwned>(&self, rb: RequestBuilder) -> Result<T, JiraError> {
        let body = self.send(rb).await?;
        serde_json::from_slice(&body).map_err(|e| JiraError::Decode(e.to_string()))
    }

    /// `GET /rest/api/2/myself` — имя пользователя токена.
    pub async fn myself(&self) -> Result<String, JiraError> {
        let v: Value = self
            .json(self.req(Method::GET, "/rest/api/2/myself"))
            .await?;
        ["displayName", "name", "emailAddress", "accountId"]
            .iter()
            .find_map(|k| v.get(k).and_then(Value::as_str))
            .map(str::to_owned)
            .ok_or_else(|| JiraError::Decode("нет displayName".into()))
    }

    /// `GET /rest/api/2/field` — все поля.
    pub async fn fields(&self) -> Result<Vec<Field>, JiraError> {
        self.json(self.req(Method::GET, "/rest/api/2/field")).await
    }

    /// `GET /rest/api/2/issue/{key}?fields=summary,<field>`.
    pub async fn issue(&self, key: &str, field: Option<&str>) -> Result<Issue, JiraError> {
        let fields = match field {
            Some(f) => format!("summary,{f}"),
            None => "summary".to_owned(),
        };
        let v: Value = self
            .json(
                self.req(Method::GET, &format!("/rest/api/2/issue/{key}"))
                    .query(&[("fields", fields)]),
            )
            .await?;
        let f = v.get("fields").cloned().unwrap_or(Value::Null);
        let checklist = match field.map(|id| f.get(id).cloned().unwrap_or(Value::Null)) {
            None | Some(Value::Null) => String::new(),
            Some(Value::String(s)) => s,
            Some(other) => {
                return Err(JiraError::Decode(format!(
                    "поле чек-листа {} не текстовое: {}",
                    field.unwrap_or_default(),
                    truncate(&other.to_string(), 80)
                )));
            }
        };
        Ok(Issue {
            key: v
                .get("key")
                .and_then(Value::as_str)
                .unwrap_or(key)
                .to_owned(),
            summary: f
                .get("summary")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
            checklist,
        })
    }

    /// `PUT /rest/api/2/issue/{key}` с текстом поля чек-листа целиком.
    pub async fn put_checklist(&self, key: &str, field: &str, text: &str) -> Result<(), JiraError> {
        let body = json!({ "fields": { field: text } });
        self.send(
            self.req(Method::PUT, &format!("/rest/api/2/issue/{key}"))
                .json(&body),
        )
        .await
        .map(drop)
    }

    /// `POST /rest/api/2/issue/{key}/worklog`; возвращает id записи.
    pub async fn add_worklog(
        &self,
        key: &str,
        started: DateTime<Utc>,
        seconds: i64,
        comment: &str,
    ) -> Result<String, JiraError> {
        let body = worklog_body(started, seconds, comment);
        let v: Value = self
            .json(
                self.req(Method::POST, &format!("/rest/api/2/issue/{key}/worklog"))
                    .json(&body),
            )
            .await?;
        Ok(match v.get("id") {
            Some(Value::String(s)) => s.clone(),
            Some(Value::Number(n)) => n.to_string(),
            _ => String::new(),
        })
    }
}

/// Тело worklog: `started` в формате Jira `yyyy-MM-dd'T'HH:mm:ss.SSSZ`,
/// длительность не меньше минуты (Jira не принимает меньше).
pub fn worklog_body(started: DateTime<Utc>, seconds: i64, comment: &str) -> Value {
    json!({
        "started": jira_time(started),
        "timeSpentSeconds": seconds.max(60),
        "comment": comment,
    })
}

/// Момент в формате Jira: `2026-10-24T09:00:00.000+0000`.
pub fn jira_time(at: DateTime<Utc>) -> String {
    at.format("%Y-%m-%dT%H:%M:%S%.3f%z").to_string()
}

/// Подходящее поле чек-листа: имя «Smart Checklist», затем содержащее
/// «smart checklist», затем «checklist» (без «progress» — служебное поле
/// прогресса Smart Checklist).
pub fn pick_checklist_field(fields: &[Field]) -> Option<&Field> {
    let low = |f: &Field| f.name.trim().to_lowercase();
    let usable = |f: &&Field| !low(f).contains("progress");
    fields
        .iter()
        .find(|f| low(f) == "smart checklist")
        .or_else(|| {
            fields
                .iter()
                .filter(usable)
                .find(|f| low(f).contains("smart checklist"))
        })
        .or_else(|| {
            fields
                .iter()
                .filter(usable)
                .find(|f| low(f).contains("checklist"))
        })
}

fn jira_messages(body: &[u8]) -> Option<String> {
    let v: Value = serde_json::from_slice(body).ok()?;
    let mut out: Vec<String> = v
        .get("errorMessages")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_owned)
        .collect();
    if let Some(errs) = v.get("errors").and_then(Value::as_object) {
        out.extend(
            errs.iter()
                .map(|(k, v)| format!("{k}: {}", v.as_str().unwrap_or_default())),
        );
    }
    (!out.is_empty()).then(|| truncate(&out.join("; "), 300))
}

fn reason(s: StatusCode) -> String {
    s.canonical_reason().unwrap_or("ошибка").to_owned()
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_owned()
    } else {
        format!("{}…", s.chars().take(n).collect::<String>())
    }
}

fn error_chain(e: &dyn std::error::Error) -> String {
    let mut s = e.to_string();
    let mut src = e.source();
    while let Some(x) = src {
        s.push_str(": ");
        s.push_str(&x.to_string());
        src = x.source();
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn time_format() {
        let t = Utc.with_ymd_and_hms(2026, 10, 24, 9, 5, 7).unwrap();
        assert_eq!(jira_time(t), "2026-10-24T09:05:07.000+0000");
        let b = worklog_body(t, 12, "x");
        assert_eq!(b["timeSpentSeconds"], 60);
    }

    #[test]
    fn picks_field() {
        let f = |id: &str, name: &str| Field {
            id: id.into(),
            name: name.into(),
        };
        let all = [
            f("c1", "Smart Checklist Progress"),
            f("c2", "My Checklist"),
            f("c3", "Smart Checklist"),
        ];
        assert_eq!(pick_checklist_field(&all).unwrap().id, "c3");
        assert_eq!(pick_checklist_field(&all[..2]).unwrap().id, "c2");
        assert!(pick_checklist_field(&all[..1]).is_none());
    }
}
