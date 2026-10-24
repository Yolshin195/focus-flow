//! [`RestError`] — [`ApiError`] как HTTP-ответ, и извлекатели [`ApiJson`],
//! [`ApiPath`], у которых ошибка разбора тоже отдаётся как `ApiError::Validation`.

use axum::Json;
use axum::extract::rejection::{JsonRejection, PathRejection};
use axum::extract::{FromRequest, FromRequestParts, Request};
use axum::http::StatusCode;
use axum::http::request::Parts;
use axum::response::{IntoResponse, Response};
use focus_api_types::ApiError;
use focus_core::CoreError;
use serde::de::DeserializeOwned;

/// Ошибка обработчика: JSON `ApiError` и статус [`ApiError::http_status`].
#[derive(Debug)]
pub struct RestError(pub ApiError);

impl From<ApiError> for RestError {
    fn from(e: ApiError) -> Self {
        Self(e)
    }
}

impl From<CoreError> for RestError {
    fn from(e: CoreError) -> Self {
        Self(e.into())
    }
}

impl IntoResponse for RestError {
    fn into_response(self) -> Response {
        let status =
            StatusCode::from_u16(self.0.http_status()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
        if status.is_server_error() {
            tracing::error!(error = %self.0, "ошибка запроса");
        }
        (status, Json(self.0)).into_response()
    }
}

/// JSON-тело; неразборчивое → `422 {"code":"validation"}`.
#[derive(Debug, Clone, Copy, Default)]
pub struct ApiJson<T>(pub T);

impl<S, T> FromRequest<S> for ApiJson<T>
where
    T: DeserializeOwned,
    S: Send + Sync,
{
    type Rejection = RestError;

    async fn from_request(req: Request, state: &S) -> Result<Self, Self::Rejection> {
        match Json::<T>::from_request(req, state).await {
            Ok(Json(v)) => Ok(Self(v)),
            Err(e) => Err(json_rejection(&e)),
        }
    }
}

fn json_rejection(e: &JsonRejection) -> RestError {
    RestError(ApiError::Validation(format!(
        "тело запроса: {}",
        e.body_text()
    )))
}

/// Параметр пути; неразборчивый (не UUID, не дата) → `422 {"code":"validation"}`.
#[derive(Debug, Clone, Copy)]
pub struct ApiPath<T>(pub T);

impl<S, T> FromRequestParts<S> for ApiPath<T>
where
    T: DeserializeOwned + Send,
    S: Send + Sync,
{
    type Rejection = RestError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        match axum::extract::Path::<T>::from_request_parts(parts, state).await {
            Ok(axum::extract::Path(v)) => Ok(Self(v)),
            Err(e) => Err(path_rejection(&e)),
        }
    }
}

fn path_rejection(e: &PathRejection) -> RestError {
    RestError(ApiError::Validation(format!("путь: {}", e.body_text())))
}
