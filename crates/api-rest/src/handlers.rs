//! Обработчики REST: тонкий слой над inherent-методами [`FocusService`].
//! Изменяющие команды публикуют `status_changed` (и `task_changed`) в шину SSE.
//!
//! [`FocusService`]: focus_core::FocusService

use axum::Json;
use axum::extract::State;
use chrono::NaiveDate;
use focus_api_types::requests::{DoneRequest, StartWorkRequest};
use focus_api_types::{
    DayDto, ImportRequest, ImportResult, NewTask, PluginStatusDto, ServerEvent, SessionEndResult,
    SettingsDto, StatusDto, TaskDto, TaskId, TaskPatch,
};
use serde_json::{Value, json};

use crate::RestState;
use crate::error::{ApiJson, ApiPath, RestError};

type Res<T> = Result<Json<T>, RestError>;

/// `GET /api/health` — без токена.
pub async fn health() -> Json<Value> {
    Json(json!({ "status": "ok", "version": env!("CARGO_PKG_VERSION") }))
}

pub async fn status(State(s): State<RestState>) -> Res<StatusDto> {
    Ok(Json(s.service.status().await?))
}

/// Ответ команды со статусом: разослать его и вернуть.
fn with_status(s: &RestState, status: StatusDto) -> Res<StatusDto> {
    s.broadcast(ServerEvent::StatusChanged(status.clone()));
    Ok(Json(status))
}

/// Ответ завершения сессии: разослать `session_ended` и новый статус.
fn with_end(s: &RestState, r: SessionEndResult) -> Res<SessionEndResult> {
    s.broadcast(ServerEvent::SessionEnded {
        session: r.ended.clone(),
    });
    s.broadcast(ServerEvent::StatusChanged(r.status.clone()));
    Ok(Json(r))
}

/// После правки задачи: `task_changed`, а если поменялся пункт в работе
/// (название) — и статус.
async fn task_changed(s: &RestState, task_id: TaskId) {
    s.broadcast(ServerEvent::TaskChanged { task_id });
    if let Ok(status) = s.service.status().await {
        s.broadcast(ServerEvent::StatusChanged(status));
    }
}

pub async fn open_day(State(s): State<RestState>) -> Res<StatusDto> {
    let st = s.service.open_day().await?;
    with_status(&s, st)
}

pub async fn close_day(State(s): State<RestState>) -> Res<StatusDto> {
    let st = s.service.close_day().await?;
    with_status(&s, st)
}

pub async fn day(State(s): State<RestState>, ApiPath(date): ApiPath<NaiveDate>) -> Res<DayDto> {
    Ok(Json(s.service.day(date).await?))
}

pub async fn list_tasks(State(s): State<RestState>) -> Res<Vec<TaskDto>> {
    Ok(Json(s.service.list_tasks().await?))
}

pub async fn create_task(
    State(s): State<RestState>,
    ApiJson(new): ApiJson<NewTask>,
) -> Result<(axum::http::StatusCode, Json<TaskDto>), RestError> {
    let task = s.service.create_task(new).await?;
    task_changed(&s, task.id).await;
    Ok((axum::http::StatusCode::CREATED, Json(task)))
}

pub async fn update_task(
    State(s): State<RestState>,
    ApiPath(id): ApiPath<TaskId>,
    ApiJson(patch): ApiJson<TaskPatch>,
) -> Res<TaskDto> {
    let task = s.service.update_task(id, patch).await?;
    task_changed(&s, task.id).await;
    Ok(Json(task))
}

pub async fn start_work(
    State(s): State<RestState>,
    ApiJson(req): ApiJson<StartWorkRequest>,
) -> Res<StatusDto> {
    let st = s.service.start_work(req.item_id).await?;
    // Пункт мог перейти planned → in_progress.
    s.broadcast(ServerEvent::TaskChanged {
        task_id: req.item_id,
    });
    with_status(&s, st)
}

pub async fn start_planning(State(s): State<RestState>) -> Res<StatusDto> {
    let st = s.service.start_planning().await?;
    with_status(&s, st)
}

pub async fn start_rest(State(s): State<RestState>) -> Res<StatusDto> {
    let st = s.service.start_rest().await?;
    with_status(&s, st)
}

pub async fn done(
    State(s): State<RestState>,
    ApiJson(req): ApiJson<DoneRequest>,
) -> Res<SessionEndResult> {
    let r = s.service.done(req.next).await?;
    if let Some(task_id) = r.ended.task_id {
        s.broadcast(ServerEvent::TaskChanged { task_id });
    }
    with_end(&s, r)
}

pub async fn interrupt(State(s): State<RestState>) -> Res<SessionEndResult> {
    let r = s.service.interrupt().await?;
    with_end(&s, r)
}

pub async fn stop(State(s): State<RestState>) -> Res<SessionEndResult> {
    let r = s.service.stop().await?;
    with_end(&s, r)
}

pub async fn import(
    State(s): State<RestState>,
    ApiJson(req): ApiJson<ImportRequest>,
) -> Res<ImportResult> {
    let r = s.service.import(&req.url).await?;
    task_changed(&s, r.task.id).await;
    Ok(Json(r))
}

pub async fn get_settings(State(s): State<RestState>) -> Res<SettingsDto> {
    Ok(Json(s.service.settings().await?))
}

pub async fn put_settings(
    State(s): State<RestState>,
    ApiJson(dto): ApiJson<SettingsDto>,
) -> Res<SettingsDto> {
    let saved = s.service.update_settings(dto).await?;
    if let Ok(status) = s.service.status().await {
        s.broadcast(ServerEvent::StatusChanged(status));
    }
    Ok(Json(saved))
}

pub async fn plugin_status(State(s): State<RestState>) -> Res<Vec<PluginStatusDto>> {
    Ok(Json(s.service.plugin_status().await?))
}
