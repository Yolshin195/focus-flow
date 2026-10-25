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
    /// Тип значения; у некоторых системных полей схемы нет.
    #[serde(default)]
    pub schema: Option<FieldSchema>,
}

impl Field {
    /// Значение поля — строка (или тип неизвестен): такое поле можно
    /// читать и писать текстом.
    pub fn is_text(&self) -> bool {
        self.schema
            .as_ref()
            .and_then(|s| s.kind.as_deref())
            .is_none_or(|k| k == "string")
    }
}

/// Схема поля Jira: `{"type": "string", "custom": "…:textarea"}`.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct FieldSchema {
    /// Тип значения: `string`, `array`, `number`, `any`…
    #[serde(default, rename = "type")]
    pub kind: Option<String>,
}

/// Свойство задачи, в котором Smart Checklist (Railsware) хранит чек-лист
/// текстом. Писать чек-лист — только сюда: поле «Checklists» — его копия,
/// обычно не на экране редактирования, и `PUT` поля отвечает 400.
pub const CHECKLIST_PROPERTY: &str = "com.railsware.SmartChecklist.checklist";

/// Задача Jira: название и текст чек-листа.
#[derive(Debug, Clone)]
pub struct Issue {
    /// Числовой id (`259396`); пустой, если Jira его не вернула.
    pub id: String,
    /// Ключ (`PROJ-1`).
    pub key: String,
    /// Название.
    pub summary: String,
    /// Текст чек-листа (`""`, если его нет).
    pub checklist: String,
}

impl Issue {
    /// Id для адресов задачи: числовой, а без него — ключ.
    pub fn id_or_key(&self) -> &str {
        if self.id.is_empty() {
            &self.key
        } else {
            &self.id
        }
    }
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

    /// `GET /rest/api/2/issue/{key}?fields=summary,<field>&properties=<чек-лист>`.
    ///
    /// Текст чек-листа — из свойства [`CHECKLIST_PROPERTY`] (его пишет Smart
    /// Checklist), без него — из поля `field` («Checklists», копия свойства).
    /// Если Jira не вернула свойства вовсе, а поля нет, свойство читается
    /// отдельным запросом: перед записью текст должен быть свежим.
    pub async fn issue(&self, key: &str, field: Option<&str>) -> Result<Issue, JiraError> {
        let fields = match field {
            Some(f) => format!("summary,{f}"),
            None => "summary".to_owned(),
        };
        let v: Value = self
            .json(
                self.req(Method::GET, &format!("/rest/api/2/issue/{key}"))
                    .query(&[
                        ("fields", fields.as_str()),
                        ("properties", CHECKLIST_PROPERTY),
                    ]),
            )
            .await?;
        let f = v.get("fields").cloned().unwrap_or(Value::Null);
        let mut issue = Issue {
            id: match v.get("id") {
                Some(Value::String(s)) => s.clone(),
                Some(Value::Number(n)) => n.to_string(),
                _ => String::new(),
            },
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
            checklist: String::new(),
        };
        let props = v.get("properties");
        issue.checklist = match (props.and_then(|p| p.get(CHECKLIST_PROPERTY)), field) {
            (Some(Value::String(text)), _) => text.clone(),
            (_, Some(id)) => checklist_text(id, f.get(id).cloned().unwrap_or(Value::Null))?,
            (_, None) if props.is_none() => self.checklist_property(issue.id_or_key()).await?,
            (_, None) => String::new(),
        };
        Ok(issue)
    }

    /// `GET /rest/api/2/issue/{id}/properties/<чек-лист>`: текст чек-листа;
    /// свойства нет (404) — чек-листа ещё не было, `""`.
    pub async fn checklist_property(&self, issue: &str) -> Result<String, JiraError> {
        let rb = self.req(
            Method::GET,
            &format!("/rest/api/2/issue/{issue}/properties/{CHECKLIST_PROPERTY}"),
        );
        let v: Value = match self.json(rb).await {
            Err(JiraError::Client { status: 404, .. }) => return Ok(String::new()),
            r => r?,
        };
        Ok(v.get("value")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned())
    }

    /// `PUT /rest/api/2/issue/{id}/properties/<чек-лист>` с текстом чек-листа
    /// целиком (тело — JSON-строка). Smart Checklist сам обновит пункты,
    /// прогресс и поле «Checklists».
    pub async fn put_checklist(&self, issue: &str, text: &str) -> Result<(), JiraError> {
        self.send(
            self.req(
                Method::PUT,
                &format!("/rest/api/2/issue/{issue}/properties/{CHECKLIST_PROPERTY}"),
            )
            .json(text),
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

/// Текст чек-листа из значения поля `id` (когда свойства чек-листа нет):
/// `null` — пустой чек-лист.
///
/// Не строка — это не то поле: у Smart Checklist для Data Center поле
/// «Smart Checklist» служит для JQL и отдаёт список объектов
/// (`["Checklist(id=…, _items=[Item(…)])"]`), а текст лежит в «Checklists».
pub fn checklist_text(id: &str, value: Value) -> Result<String, JiraError> {
    match value {
        Value::Null => Ok(String::new()),
        Value::String(s) => Ok(s),
        other => Err(JiraError::Decode(format!(
            "поле чек-листа {id} не текстовое — нужно текстовое поле «Checklists» \
             (укажите его id в настройках Jira или оставьте поле пустым для автопоиска): {}",
            truncate(&other.to_string(), 80)
        ))),
    }
}

/// Подходящее поле чек-листа. Только текстовые поля без «progress» в имени
/// (служебное поле прогресса Smart Checklist), по порядку: «Checklists» —
/// текстовое поле Smart Checklist для REST API, затем «Smart Checklist»,
/// затем содержащее «smart checklist», затем «checklist».
pub fn pick_checklist_field(fields: &[Field]) -> Option<&Field> {
    let rank = |f: &Field| {
        let name = f.name.trim().to_lowercase();
        if !f.is_text() || name.contains("progress") {
            return None;
        }
        match name.as_str() {
            "checklists" => Some(0),
            "smart checklist" => Some(1),
            n if n.contains("smart checklist") => Some(2),
            n if n.contains("checklist") => Some(3),
            _ => None,
        }
    };
    fields
        .iter()
        .filter_map(|f| rank(f).map(|r| (r, f)))
        .min_by_key(|(r, _)| *r)
        .map(|(_, f)| f)
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

    fn field(id: &str, name: &str, kind: Option<&str>) -> Field {
        Field {
            id: id.into(),
            name: name.into(),
            schema: Some(FieldSchema {
                kind: kind.map(str::to_owned),
            }),
        }
    }

    #[test]
    fn picks_field() {
        let f = |id, name| field(id, name, Some("string"));
        let all = [
            f("c1", "Smart Checklist Progress"),
            f("c2", "My Checklist"),
            f("c3", "Smart Checklist"),
            f("c4", "Checklists"),
        ];
        assert_eq!(pick_checklist_field(&all).unwrap().id, "c4");
        assert_eq!(pick_checklist_field(&all[..3]).unwrap().id, "c3");
        assert_eq!(pick_checklist_field(&all[..2]).unwrap().id, "c2");
        assert!(pick_checklist_field(&all[..1]).is_none());
    }

    #[test]
    fn skips_non_text_fields() {
        // Smart Checklist для Data Center: «Smart Checklist» — не текст.
        let dc = [
            field("customfield_11600", "Smart Checklist", Some("array")),
            field(
                "customfield_11601",
                "Smart Checklist Progress",
                Some("string"),
            ),
            field("customfield_11602", "Checklists", Some("string")),
        ];
        assert_eq!(pick_checklist_field(&dc).unwrap().id, "customfield_11602");
        assert!(pick_checklist_field(&dc[..2]).is_none());
        // Без схемы — тип неизвестен, поле годится.
        let bare: Field =
            serde_json::from_value(json!({"id": "c1", "name": "Checklists"})).unwrap();
        assert!(bare.is_text());
        assert_eq!(pick_checklist_field(&[bare]).unwrap().id, "c1");
    }

    #[test]
    fn checklist_value() {
        assert_eq!(checklist_text("c", Value::Null).unwrap(), "");
        assert_eq!(checklist_text("c", json!("- a")).unwrap(), "- a");
        let err = checklist_text(
            "customfield_11600",
            json!(["Checklist(id=184286, issueId=260721, _items=[Item(id=15355)])"]),
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("customfield_11600"), "{err}");
        assert!(err.contains("«Checklists»"), "{err}");
    }
}
