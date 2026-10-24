//! Трейт [`FocusApi`] — все операции сервиса. Его реализуют `HttpApi`
//! и `MockApi` в клиенте, а сценарии `focus-api-contract-tests` проверяют
//! любую реализацию.

use async_trait::async_trait;
use chrono::NaiveDate;

use crate::day::{DayDto, StatusDto};
use crate::error::ApiError;
use crate::ids::TaskId;
use crate::session::{DoneNext, SessionEndResult};
use crate::settings::{PluginStatusDto, SettingsDto};
use crate::task::{ImportResult, NewTask, TaskDto, TaskPatch};

/// Результат операции API.
pub type ApiResult<T> = Result<T, ApiError>;

/// Операции сервиса: по методу на каждую строку таблицы «REST API» в SPEC.
///
/// `?Send` — чтобы трейт реализовывался в wasm (браузерные future не `Send`).
/// События приходят отдельно, через SSE `GET /api/events` как [`crate::ServerEvent`];
/// метода подписки в трейте нет, чтобы не тянуть сюда runtime и `futures`.
#[async_trait(?Send)]
pub trait FocusApi {
    /// `GET /api/status` — текущее состояние.
    async fn get_status(&self) -> ApiResult<StatusDto>;

    /// `POST /api/day/open` — открыть сегодняшний день (или снова открыть закрытый).
    /// Если день уже открыт — ничего не меняет.
    async fn open_day(&self) -> ApiResult<StatusDto>;

    /// `POST /api/day/close` — закрыть сегодняшний день.
    /// При активной сессии — [`ApiError::SessionAlreadyActive`].
    async fn close_day(&self) -> ApiResult<StatusDto>;

    /// `GET /api/days/{date}` — сессии и итоги дня для календаря.
    async fn get_day(&self, date: NaiveDate) -> ApiResult<DayDto>;

    /// `GET /api/tasks` — дерево задач (узлы верхнего уровня с `children`).
    async fn list_tasks(&self) -> ApiResult<Vec<TaskDto>>;

    /// `POST /api/tasks` — создать задачу или пункт.
    async fn create_task(&self, task: NewTask) -> ApiResult<TaskDto>;

    /// `PATCH /api/tasks/{id}` — частичная правка.
    async fn update_task(&self, id: TaskId, patch: TaskPatch) -> ApiResult<TaskDto>;

    /// `POST /api/sessions/work` — начать работу над пунктом.
    /// Автооткрывает день. Активное планирование закрывается с `switched`.
    async fn start_work(&self, item_id: TaskId) -> ApiResult<StatusDto>;

    /// `POST /api/sessions/planning` — начать планирование.
    async fn start_planning(&self) -> ApiResult<StatusDto>;

    /// `POST /api/sessions/rest` — начать отдых. Активное планирование
    /// закрывается с `switched`.
    async fn start_rest(&self) -> ApiResult<StatusDto>;

    /// `POST /api/sessions/active/done` — «Готово»: закрыть работу, пункт → `done`,
    /// сразу начать `next`.
    async fn done(&self, next: DoneNext) -> ApiResult<SessionEndResult>;

    /// `POST /api/sessions/active/interrupt` — «Прервать» работу (`interrupted`).
    async fn interrupt(&self) -> ApiResult<SessionEndResult>;

    /// `POST /api/sessions/active/stop` — «Стоп» планирования (`stopped`).
    async fn stop(&self) -> ApiResult<SessionEndResult>;

    /// `POST /api/import` — импорт задачи по ссылке через подходящий `TaskSource`.
    async fn import(&self, url: String) -> ApiResult<ImportResult>;

    /// `GET /api/settings`.
    async fn get_settings(&self) -> ApiResult<SettingsDto>;

    /// `PUT /api/settings` — заменить настройки; возвращает сохранённые.
    async fn update_settings(&self, settings: SettingsDto) -> ApiResult<SettingsDto>;

    /// `GET /api/plugins/status` — состояние плагинов и очереди outbox.
    async fn plugin_status(&self) -> ApiResult<Vec<PluginStatusDto>>;
}
