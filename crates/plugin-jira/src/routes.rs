//! Маршруты плагина (сервер вкладывает их в `/api/plugins/jira`):
//! - `GET /settings` — настройки и состояние; токен не возвращается, только `has_token`;
//! - `PUT /settings` — частичная правка: переданные поля заменяются, `null`
//!   очищает; `token` (только запись) кладётся в секреты, `""` — удаляет его;
//! - `POST /test` — проверить подключение: `GET /rest/api/2/myself`;
//! - `GET /queue` — очередь отправки: `{auto_send, items: [Op]}`;
//! - `POST /queue/send` — отправить её сейчас: то же + `sent`, `error`;
//! - `DELETE /queue/{id}` — не отправлять запись (повтор события её не вернёт);
//! - `POST /sync` — забрать изменения из Jira сейчас: [`SyncResult`](crate::SyncResult).

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use focus_api_types::ApiError;
use focus_plugin_api::PluginCtx;
use serde_json::{Map, Value, json};

use crate::JiraPlugin;
use crate::client::Jira;
use crate::queue;
use crate::settings::{JiraSettings, TOKEN_SECRET, validate_base_url};

type St = (JiraPlugin, PluginCtx);

/// Ошибка маршрута — JSON `ApiError` со статусом `http_status()`.
struct RouteError(ApiError);

impl IntoResponse for RouteError {
    fn into_response(self) -> Response {
        let code =
            StatusCode::from_u16(self.0.http_status()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
        (code, Json(self.0)).into_response()
    }
}

impl From<anyhow::Error> for RouteError {
    fn from(e: anyhow::Error) -> Self {
        Self(ApiError::Internal(format!("{e:#}")))
    }
}

type Res = Result<Json<Value>, RouteError>;

pub(crate) fn router(plugin: JiraPlugin, ctx: PluginCtx) -> Router {
    Router::new()
        .route("/settings", get(get_settings).put(put_settings))
        .route("/test", post(test))
        .route("/queue", get(get_queue))
        .route("/queue/send", post(send_queue))
        .route("/queue/{id}", delete(drop_queued))
        .route("/sync", post(sync))
        .with_state((plugin, ctx))
}

async fn get_settings(State((p, ctx)): State<St>) -> Res {
    Ok(Json(p.state_json(&ctx).await?))
}

const STRING_KEYS: [&str; 3] = ["base_url", "email", "checklist_field_id"];
const VALUE_KEYS: [&str; 5] = ["mode", "worklog", "enabled", "auto_send", "auto_sync"];

async fn put_settings(State((p, ctx)): State<St>, body: axum::body::Bytes) -> Res {
    let bad = |m: String| RouteError(ApiError::Validation(m));
    let patch: Map<String, Value> =
        serde_json::from_slice(&body).map_err(|e| bad(format!("тело: {e}")))?;
    let mut cur = match ctx.settings().get().await? {
        Value::Object(m) => m,
        _ => Map::new(),
    };
    let mut token: Option<Option<String>> = None;
    for (k, v) in patch {
        match (k.as_str(), v) {
            ("token", Value::Null) => token = Some(None),
            ("token", Value::String(t)) => {
                let t = t.trim().to_owned();
                token = Some((!t.is_empty()).then_some(t));
            }
            ("token", _) => return Err(bad("token: нужна строка".into())),
            (k, Value::Null) if STRING_KEYS.contains(&k) || VALUE_KEYS.contains(&k) => {
                cur.remove(k);
            }
            (k, Value::String(s)) if STRING_KEYS.contains(&k) => {
                let s = s.trim();
                if s.is_empty() {
                    cur.remove(k);
                } else {
                    let s = if k == "base_url" {
                        validate_base_url(s).map_err(bad)?
                    } else {
                        s.to_owned()
                    };
                    cur.insert(k.to_owned(), Value::String(s));
                }
            }
            (k, _) if STRING_KEYS.contains(&k) => return Err(bad(format!("{k}: нужна строка"))),
            (k, v) if VALUE_KEYS.contains(&k) => {
                cur.insert(k.to_owned(), v);
            }
            (k, _) => return Err(bad(format!("неизвестное поле «{k}»"))),
        }
    }
    let merged = Value::Object(cur);
    // Проверка схемы целиком (mode, worklog, enabled).
    JiraSettings::from_value(&merged).map_err(bad)?;
    match token {
        Some(Some(t)) => ctx.secrets().set(TOKEN_SECRET, &t)?,
        Some(None) => ctx.secrets().delete(TOKEN_SECRET)?,
        None => {}
    }
    ctx.settings().set(&merged).await?;
    Ok(Json(p.state_json(&ctx).await?))
}

async fn test(State((p, ctx)): State<St>) -> Res {
    let (_, conn) = p.conn(&ctx).await?;
    let conn =
        conn.map_err(|why| RouteError(ApiError::Validation(format!("Jira не настроена: {why}"))))?;
    let jira = Jira::new(&p.inner.http, &conn);
    match p.note(jira.myself().await) {
        Ok(user) => Ok(Json(
            json!({"ok": true, "user": user, "base_url": conn.base}),
        )),
        Err(e) => Err(RouteError(ApiError::PluginUnavailable(e.to_string()))),
    }
}

async fn get_queue(State((p, ctx)): State<St>) -> Res {
    Ok(Json(p.queue_json(&ctx).await?))
}

async fn send_queue(State((p, ctx)): State<St>) -> Res {
    let (_, conn) = p.conn(&ctx).await?;
    if let Err(why) = conn {
        return Err(RouteError(ApiError::Validation(format!(
            "Jira не настроена: {why}"
        ))));
    }
    let r = p.flush(&ctx).await?;
    let mut v = p.queue_json(&ctx).await?;
    v["sent"] = json!(r.sent);
    v["error"] = json!(r.error);
    Ok(Json(v))
}

async fn sync(State((p, ctx)): State<St>) -> Res {
    let (_, conn) = p.conn(&ctx).await?;
    if let Err(why) = conn {
        return Err(RouteError(ApiError::Validation(format!(
            "Jira не настроена: {why}"
        ))));
    }
    let r = p.sync(&ctx).await?;
    Ok(Json(serde_json::to_value(r).map_err(anyhow::Error::from)?))
}

async fn drop_queued(State((p, ctx)): State<St>, Path(id): Path<i64>) -> Res {
    if queue::drop_op(ctx.pool(), id)
        .await
        .map_err(anyhow::Error::from)?
    {
        tracing::info!(id, "запись убрана из очереди Jira");
    }
    Ok(Json(p.queue_json(&ctx).await?))
}
