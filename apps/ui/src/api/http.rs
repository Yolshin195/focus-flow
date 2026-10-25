//! [`HttpApi`] — `FocusApi` поверх REST сервиса (reqwest: на desktop — hyper,
//! в wasm — `fetch`), и поток событий SSE `GET /api/events`.
//!
//! Ошибки: ответ не 2xx → [`ApiError`] из JSON-тела (`{"code","details"}`);
//! сеть, таймаут, неразборчивый ответ → [`ApiError::Transport`].

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use async_trait::async_trait;
use focus_api_types::chrono::NaiveDate;
use focus_api_types::requests::{DoneRequest, StartWorkRequest};
use focus_api_types::{
    ApiError, ApiResult, DayDto, DoneNext, FocusApi, ImportRequest, ImportResult, NewTask,
    PluginStatusDto, ServerEvent, SessionEndResult, SettingsDto, StatusDto, TaskDto, TaskId,
    TaskPatch,
};
use futures_util::{Stream, StreamExt};
use reqwest::{Method, RequestBuilder, StatusCode};
use serde::de::DeserializeOwned;
use serde::Serialize;

use super::jira::{
    JiraPatch, JiraQueue, JiraSettingsApi, JiraState, JiraSyncResult, JiraTestResult,
};

/// Таймаут обычного запроса. Сервис локальный — дольше ждать незачем.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

/// Откуда брать токен: строка или функция (например, чтение файла `token`,
/// который появляется только после первого запуска сервиса).
type TokenSource = Rc<dyn Fn() -> Option<String>>;

/// Клиент REST API сервиса Focus Flow.
#[derive(Clone)]
pub struct HttpApi {
    base: String,
    client: reqwest::Client,
    source: TokenSource,
    /// Последний прочитанный токен; сбрасывается на `401`, чтобы перечитать.
    token: Rc<RefCell<Option<String>>>,
}

impl HttpApi {
    /// Клиент с постоянным токеном. `base_url` — `http://127.0.0.1:7878`
    /// (схему можно не писать).
    pub fn new(base_url: impl Into<String>, token: impl Into<String>) -> Self {
        let token = token.into();
        Self::with_token_source(base_url, move || Some(token.clone()))
    }

    /// Клиент, который берёт токен из `source` при первом запросе и
    /// повторно — после ответа `401`.
    pub fn with_token_source(
        base_url: impl Into<String>,
        source: impl Fn() -> Option<String> + 'static,
    ) -> Self {
        Self {
            base: normalize_base(&base_url.into()),
            client: reqwest::Client::new(),
            source: Rc::new(source),
            token: Rc::new(RefCell::new(None)),
        }
    }

    /// Базовый адрес без `/` на конце.
    pub fn base_url(&self) -> &str {
        &self.base
    }

    fn token(&self) -> String {
        let mut cached = self.token.borrow_mut();
        if cached.is_none() {
            *cached = (self.source)().map(|t| t.trim().to_owned());
        }
        cached.clone().unwrap_or_default()
    }

    fn request(&self, method: Method, path: &str) -> RequestBuilder {
        self.client
            .request(method, format!("{}{path}", self.base))
            .bearer_auth(self.token())
    }

    async fn send<T: DeserializeOwned>(&self, req: RequestBuilder) -> ApiResult<T> {
        let resp = req
            .timeout(REQUEST_TIMEOUT)
            .send()
            .await
            .map_err(transport)?;
        let status = resp.status();
        let body = resp.bytes().await.map_err(transport)?;
        if status.is_success() {
            return serde_json::from_slice(&body)
                .map_err(|e| ApiError::Transport(format!("неразборчивый ответ: {e}")));
        }
        let err = decode_error(status, &body);
        if err == ApiError::Unauthorized {
            // Токен мог смениться (сервис пересоздал файл) — перечитать в следующий раз.
            self.token.borrow_mut().take();
        }
        Err(err)
    }

    pub(crate) async fn get<T: DeserializeOwned>(&self, path: &str) -> ApiResult<T> {
        self.send(self.request(Method::GET, path)).await
    }

    pub(crate) async fn post<T: DeserializeOwned>(&self, path: &str) -> ApiResult<T> {
        self.send(self.request(Method::POST, path)).await
    }

    pub(crate) async fn delete<T: DeserializeOwned>(&self, path: &str) -> ApiResult<T> {
        self.send(self.request(Method::DELETE, path)).await
    }

    async fn post_json<B: Serialize, T: DeserializeOwned>(
        &self,
        path: &str,
        b: &B,
    ) -> ApiResult<T> {
        self.send(self.request(Method::POST, path).json(b)).await
    }

    pub(crate) async fn put_json<B: Serialize, T: DeserializeOwned>(
        &self,
        path: &str,
        b: &B,
    ) -> ApiResult<T> {
        self.send(self.request(Method::PUT, path).json(b)).await
    }

    /// Открыть поток SSE `GET /api/events`. Элемент потока — события из
    /// очередного куска данных (пустой вектор — keep-alive `: ping`, он
    /// приходит раз в 15 с и годится для проверки «соединение живо»).
    /// Первым событием сервис присылает текущий `status_changed`. Поток
    /// кончается, когда рвётся соединение.
    pub async fn events(&self) -> ApiResult<impl Stream<Item = ApiResult<Vec<ServerEvent>>>> {
        let resp = self
            .request(Method::GET, "/api/events")
            .header(reqwest::header::ACCEPT, "text/event-stream")
            .send()
            .await
            .map_err(transport)?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.bytes().await.unwrap_or_default();
            let err = decode_error(status, &body);
            if err == ApiError::Unauthorized {
                self.token.borrow_mut().take();
            }
            return Err(err);
        }
        let mut parser = SseParser::default();
        Ok(resp.bytes_stream().map(move |chunk| match chunk {
            Ok(bytes) => Ok(parser.push(&bytes)),
            Err(e) => Err(transport(e)),
        }))
    }
}

fn normalize_base(s: &str) -> String {
    let s = s.trim().trim_end_matches('/');
    if s.contains("://") {
        s.to_owned()
    } else {
        format!("http://{s}")
    }
}

fn transport(e: reqwest::Error) -> ApiError {
    ApiError::Transport(e.to_string())
}

/// `ApiError` из тела ответа; если тело не JSON ошибки — по статусу.
fn decode_error(status: StatusCode, body: &[u8]) -> ApiError {
    if let Ok(e) = serde_json::from_slice::<ApiError>(body) {
        return e;
    }
    let text = String::from_utf8_lossy(body).trim().to_owned();
    match status {
        StatusCode::UNAUTHORIZED => ApiError::Unauthorized,
        StatusCode::FORBIDDEN => ApiError::Forbidden(text),
        StatusCode::NOT_FOUND => ApiError::Transport(format!("нет такого пути (404): {text}")),
        s => ApiError::Internal(format!("HTTP {}: {text}", s.as_u16())),
    }
}

/// Минимальный разбор `text/event-stream`: события разделены пустой строкой,
/// поле `data:` (может повторяться) — JSON [`ServerEvent`]; `event:`, `id:`,
/// `retry:` и комментарии `:` пропускаются — тип есть в самом JSON.
#[derive(Debug, Default)]
pub(crate) struct SseParser {
    buf: Vec<u8>,
}

impl SseParser {
    /// Добавить кусок потока; вернуть события, которые в нём завершились.
    pub(crate) fn push(&mut self, chunk: &[u8]) -> Vec<ServerEvent> {
        self.buf.extend_from_slice(chunk);
        let mut out = Vec::new();
        while let Some((end, sep)) = find_event_end(&self.buf) {
            let block: Vec<u8> = self.buf.drain(..end + sep).take(end).collect();
            if let Some(ev) = parse_block(&String::from_utf8_lossy(&block)) {
                out.push(ev);
            }
        }
        out
    }
}

/// Позиция конца первого события и длина разделителя (`\n\n`, `\r\n\r\n`).
fn find_event_end(buf: &[u8]) -> Option<(usize, usize)> {
    let mut i = 0;
    while i < buf.len() {
        if buf[i..].starts_with(b"\r\n\r\n") {
            return Some((i, 4));
        }
        if buf[i..].starts_with(b"\n\n") || buf[i..].starts_with(b"\r\r") {
            return Some((i, 2));
        }
        i += 1;
    }
    None
}

fn parse_block(block: &str) -> Option<ServerEvent> {
    let mut data = String::new();
    for line in block.lines() {
        let line = line.trim_end_matches('\r');
        if let Some(v) = line.strip_prefix("data:") {
            if !data.is_empty() {
                data.push('\n');
            }
            data.push_str(v.strip_prefix(' ').unwrap_or(v));
        }
    }
    if data.is_empty() {
        return None; // keep-alive `: ping`
    }
    serde_json::from_str(&data).ok()
}

#[async_trait(?Send)]
impl JiraSettingsApi for HttpApi {
    async fn jira_settings(&self) -> ApiResult<JiraState> {
        self.get("/api/plugins/jira/settings").await
    }
    async fn save_jira_settings(&self, patch: JiraPatch) -> ApiResult<JiraState> {
        self.put_json("/api/plugins/jira/settings", &patch).await
    }
    async fn test_jira(&self) -> ApiResult<JiraTestResult> {
        self.post("/api/plugins/jira/test").await
    }
    async fn jira_queue(&self) -> ApiResult<JiraQueue> {
        self.get("/api/plugins/jira/queue").await
    }
    async fn send_jira_queue(&self) -> ApiResult<JiraQueue> {
        self.post("/api/plugins/jira/queue/send").await
    }
    async fn drop_jira_op(&self, id: i64) -> ApiResult<JiraQueue> {
        self.delete(&format!("/api/plugins/jira/queue/{id}")).await
    }
    async fn sync_jira(&self) -> ApiResult<JiraSyncResult> {
        self.post("/api/plugins/jira/sync").await
    }
}

#[async_trait(?Send)]
impl FocusApi for HttpApi {
    async fn get_status(&self) -> ApiResult<StatusDto> {
        self.get("/api/status").await
    }
    async fn open_day(&self) -> ApiResult<StatusDto> {
        self.post("/api/day/open").await
    }
    async fn close_day(&self) -> ApiResult<StatusDto> {
        self.post("/api/day/close").await
    }
    async fn get_day(&self, date: NaiveDate) -> ApiResult<DayDto> {
        self.get(&format!("/api/days/{}", date.format("%Y-%m-%d")))
            .await
    }
    async fn list_tasks(&self) -> ApiResult<Vec<TaskDto>> {
        self.get("/api/tasks").await
    }
    async fn create_task(&self, task: NewTask) -> ApiResult<TaskDto> {
        self.post_json("/api/tasks", &task).await
    }
    async fn update_task(&self, id: TaskId, patch: TaskPatch) -> ApiResult<TaskDto> {
        let req = self
            .request(Method::PATCH, &format!("/api/tasks/{id}"))
            .json(&patch);
        self.send(req).await
    }
    async fn start_work(&self, item_id: TaskId) -> ApiResult<StatusDto> {
        self.post_json("/api/sessions/work", &StartWorkRequest { item_id })
            .await
    }
    async fn start_planning(&self) -> ApiResult<StatusDto> {
        self.post("/api/sessions/planning").await
    }
    async fn start_rest(&self) -> ApiResult<StatusDto> {
        self.post("/api/sessions/rest").await
    }
    async fn done(&self, next: DoneNext) -> ApiResult<SessionEndResult> {
        self.post_json("/api/sessions/active/done", &DoneRequest { next })
            .await
    }
    async fn interrupt(&self) -> ApiResult<SessionEndResult> {
        self.post("/api/sessions/active/interrupt").await
    }
    async fn stop(&self) -> ApiResult<SessionEndResult> {
        self.post("/api/sessions/active/stop").await
    }
    async fn import(&self, url: String) -> ApiResult<ImportResult> {
        self.post_json("/api/import", &ImportRequest { url }).await
    }
    async fn get_settings(&self) -> ApiResult<SettingsDto> {
        self.get("/api/settings").await
    }
    async fn update_settings(&self, settings: SettingsDto) -> ApiResult<SettingsDto> {
        let req = self.request(Method::PUT, "/api/settings").json(&settings);
        self.send(req).await
    }
    async fn plugin_status(&self) -> ApiResult<Vec<PluginStatusDto>> {
        self.get("/api/plugins/status").await
    }
}

#[cfg(test)]
mod tests {
    use focus_api_types::{SessionKind, TaskId, Uuid};

    use super::*;

    #[test]
    fn sse_parser_handles_split_chunks_and_pings() {
        let id = TaskId::from(Uuid::from_u128(7));
        let json = serde_json::to_string(&ServerEvent::TaskChanged { task_id: id }).unwrap();
        let raw = format!(": ping\n\nevent: task_changed\ndata: {json}\n\nevent: x\ndata: {{\"type\":\"future_thing\"}}\n\n");
        let (a, b) = raw.as_bytes().split_at(20);
        let mut p = SseParser::default();
        let mut evs = p.push(a);
        evs.extend(p.push(b));
        assert_eq!(
            evs,
            vec![
                ServerEvent::TaskChanged { task_id: id },
                ServerEvent::Unknown
            ]
        );
        assert!(p.buf.is_empty());
    }

    #[test]
    fn sse_parser_crlf_and_multiline_data() {
        let mut p = SseParser::default();
        let evs = p.push(b"data: {\"type\":\r\ndata: \"unknown_x\"}\r\n\r\n");
        assert_eq!(evs, vec![ServerEvent::Unknown]);
    }

    #[test]
    fn errors_are_decoded_from_json() {
        let body = serde_json::to_vec(&ApiError::WrongSessionKind {
            expected: SessionKind::Planning,
            actual: SessionKind::Work,
        })
        .unwrap();
        assert_eq!(
            decode_error(StatusCode::CONFLICT, &body),
            ApiError::WrongSessionKind {
                expected: SessionKind::Planning,
                actual: SessionKind::Work
            }
        );
        assert_eq!(
            decode_error(StatusCode::UNAUTHORIZED, b"nope"),
            ApiError::Unauthorized
        );
        assert!(matches!(
            decode_error(StatusCode::BAD_GATEWAY, b"proxy"),
            ApiError::Internal(_)
        ));
    }

    #[test]
    fn base_url_is_normalized() {
        assert_eq!(normalize_base("127.0.0.1:7878"), "http://127.0.0.1:7878");
        assert_eq!(
            normalize_base("https://focus.example.com/"),
            "https://focus.example.com"
        );
    }
}
