//! Общее для тестов REST: сервис на SQLite в памяти с управляемыми часами,
//! вызов роутера без сети и реализация `FocusApi` поверх роутера.

#![allow(dead_code)]

use std::sync::Arc;

use async_trait::async_trait;
use axum::Router;
use axum::body::Body;
use axum::http::{Method, Request, Response, StatusCode, header};
use chrono::{DateTime, Duration, NaiveDate, TimeZone, Utc};
use focus_api_contract_tests::TestClock;
use focus_api_rest::{Access, RestState, router};
use focus_api_types::requests::{DoneRequest, StartWorkRequest};
use focus_api_types::{
    ApiError, ApiResult, DayDto, DoneNext, FocusApi, ImportRequest, ImportResult, NewTask,
    PluginStatusDto, SessionEndResult, SettingsDto, StatusDto, TaskDto, TaskId, TaskPatch,
};
use focus_core::{FocusService, ManualClock, NoTaskSources};
use focus_storage::Storage;
use http_body_util::BodyExt as _;
use serde::Serialize;
use serde::de::DeserializeOwned;
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;
use tower::ServiceExt as _;

pub const TOKEN: &str = "test-token";

pub fn t0() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 10, 24, 9, 0, 0).unwrap()
}

pub struct Env {
    pub clock: Arc<ManualClock>,
    pub state: RestState,
    pub router: Router,
}

impl Env {
    pub async fn new() -> Self {
        let clock = Arc::new(ManualClock::new(t0()));
        let storage = Storage::open_in_memory().await.unwrap();
        let service = Arc::new(FocusService::new(
            storage.ports(clock.clone(), Arc::new(NoTaskSources)),
        ));
        let (events, _) = broadcast::channel(64);
        let state = RestState {
            service,
            events,
            access: Arc::new(Access::new(TOKEN)),
            shutdown: CancellationToken::new(),
        };
        let router = router(state.clone());
        Self {
            clock,
            state,
            router,
        }
    }

    /// Запрос с правильными `Host` и токеном.
    pub fn req(&self, method: Method, uri: &str, body: Option<serde_json::Value>) -> Request<Body> {
        build(method, uri, body.map(|b| b.to_string()), Some(TOKEN))
    }

    pub async fn send(&self, req: Request<Body>) -> Response<Body> {
        self.router.clone().oneshot(req).await.unwrap()
    }

    /// Запрос → (статус, JSON тела).
    pub async fn call(
        &self,
        method: Method,
        uri: &str,
        body: Option<serde_json::Value>,
    ) -> (StatusCode, serde_json::Value) {
        let resp = self.send(self.req(method, uri, body)).await;
        let status = resp.status();
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let json = if bytes.is_empty() {
            serde_json::Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or_else(|e| {
                panic!(
                    "{uri}: тело не JSON ({e}): {}",
                    String::from_utf8_lossy(&bytes)
                )
            })
        };
        (status, json)
    }
}

pub fn build(
    method: Method,
    uri: &str,
    body: Option<String>,
    token: Option<&str>,
) -> Request<Body> {
    let mut b = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::HOST, "127.0.0.1:7878");
    if let Some(t) = token {
        b = b.header(header::AUTHORIZATION, format!("Bearer {t}"));
    }
    match body {
        Some(json) => b
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(json))
            .unwrap(),
        None => b.body(Body::empty()).unwrap(),
    }
}

/// `FocusApi` поверх роутера: каждый вызов — настоящий HTTP-запрос через
/// весь стек (проверки доступа, JSON, коды ошибок), но без сети.
pub struct RouterApi<'a>(pub &'a Env);

impl RouterApi<'_> {
    async fn go<T: DeserializeOwned>(
        &self,
        method: Method,
        uri: &str,
        body: Option<impl Serialize>,
    ) -> ApiResult<T> {
        let body = body.map(|b| serde_json::to_string(&b).unwrap());
        let resp = self.0.send(build(method, uri, body, Some(TOKEN))).await;
        let status = resp.status();
        let bytes = resp
            .into_body()
            .collect()
            .await
            .map_err(|e| ApiError::Transport(e.to_string()))?
            .to_bytes();
        if status.is_success() {
            serde_json::from_slice(&bytes).map_err(|e| ApiError::Transport(e.to_string()))
        } else {
            let err: ApiError =
                serde_json::from_slice(&bytes).map_err(|e| ApiError::Transport(e.to_string()))?;
            assert_eq!(
                status.as_u16(),
                err.http_status(),
                "{uri}: статус ≠ коду ошибки"
            );
            Err(err)
        }
    }

    async fn get<T: DeserializeOwned>(&self, uri: &str) -> ApiResult<T> {
        self.go(Method::GET, uri, None::<()>).await
    }

    async fn post<T: DeserializeOwned>(
        &self,
        uri: &str,
        body: Option<impl Serialize>,
    ) -> ApiResult<T> {
        self.go(Method::POST, uri, body).await
    }
}

#[async_trait(?Send)]
impl FocusApi for RouterApi<'_> {
    async fn get_status(&self) -> ApiResult<StatusDto> {
        self.get("/api/status").await
    }
    async fn open_day(&self) -> ApiResult<StatusDto> {
        self.post("/api/day/open", None::<()>).await
    }
    async fn close_day(&self) -> ApiResult<StatusDto> {
        self.post("/api/day/close", None::<()>).await
    }
    async fn get_day(&self, date: NaiveDate) -> ApiResult<DayDto> {
        self.get(&format!("/api/days/{date}")).await
    }
    async fn list_tasks(&self) -> ApiResult<Vec<TaskDto>> {
        self.get("/api/tasks").await
    }
    async fn create_task(&self, task: NewTask) -> ApiResult<TaskDto> {
        self.post("/api/tasks", Some(task)).await
    }
    async fn update_task(&self, id: TaskId, patch: TaskPatch) -> ApiResult<TaskDto> {
        self.go(Method::PATCH, &format!("/api/tasks/{id}"), Some(patch))
            .await
    }
    async fn start_work(&self, item_id: TaskId) -> ApiResult<StatusDto> {
        self.post("/api/sessions/work", Some(StartWorkRequest { item_id }))
            .await
    }
    async fn start_planning(&self) -> ApiResult<StatusDto> {
        self.post("/api/sessions/planning", None::<()>).await
    }
    async fn start_rest(&self) -> ApiResult<StatusDto> {
        self.post("/api/sessions/rest", None::<()>).await
    }
    async fn done(&self, next: DoneNext) -> ApiResult<SessionEndResult> {
        self.post("/api/sessions/active/done", Some(DoneRequest { next }))
            .await
    }
    async fn interrupt(&self) -> ApiResult<SessionEndResult> {
        self.post("/api/sessions/active/interrupt", None::<()>)
            .await
    }
    async fn stop(&self) -> ApiResult<SessionEndResult> {
        self.post("/api/sessions/active/stop", None::<()>).await
    }
    async fn import(&self, url: String) -> ApiResult<ImportResult> {
        self.post("/api/import", Some(ImportRequest { url })).await
    }
    async fn get_settings(&self) -> ApiResult<SettingsDto> {
        self.get("/api/settings").await
    }
    async fn update_settings(&self, settings: SettingsDto) -> ApiResult<SettingsDto> {
        self.go(Method::PUT, "/api/settings", Some(settings)).await
    }
    async fn plugin_status(&self) -> ApiResult<Vec<PluginStatusDto>> {
        self.get("/api/plugins/status").await
    }
}

pub struct EnvClock(pub Arc<ManualClock>);

impl TestClock for EnvClock {
    fn advance(&self, by: Duration) {
        self.0.advance(by);
    }
}
