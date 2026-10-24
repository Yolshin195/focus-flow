//! Проверки доступа для всех путей `/api/*`, по порядку:
//!
//! 1. **`Host`** — `127.0.0.1`, `localhost`, `[::1]` (с любым портом) или
//!    имя из [`Access::allowed_hosts`]; иначе `403 forbidden`. Защита от DNS
//!    rebinding: страница с чужого домена, «перепривязанного» на 127.0.0.1,
//!    шлёт `Host: evil.example`.
//! 2. **`Origin`** (если есть) — встроенные источники desktop webview
//!    (`dioxus://…`, `tauri://…`, `wry://…`, `http(s)://dioxus.localhost`,
//!    `http(s)://tauri.localhost`, `null`), loopback-страницы
//!    (`http://127.0.0.1:*`, `http://localhost:*` — dev-сервер web-клиента)
//!    и [`Access::allowed_origins`]; иначе `403 forbidden`. Для разрешённого
//!    `Origin` ответ получает CORS-заголовки, preflight `OPTIONS` отвечает `204`.
//! 3. **Токен** — `Authorization: Bearer <token>`; для `GET /api/events` ещё
//!    `?token=<token>` (браузерный `EventSource` не умеет заголовки). Без
//!    токена — только `GET /api/health`. Иначе `401 unauthorized`.

use std::sync::Arc;

use axum::extract::{Request, State};
use axum::http::header::{self, HeaderMap, HeaderValue};
use axum::http::{Method, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use focus_api_types::ApiError;

use crate::error::RestError;

/// Хосты, разрешённые всегда.
const LOOPBACK_HOSTS: [&str; 3] = ["127.0.0.1", "localhost", "[::1]"];
/// Схемы custom protocol desktop webview (любая страница схемы).
const WEBVIEW_SCHEMES: [&str; 3] = ["dioxus://", "tauri://", "wry://"];
/// Хосты, которыми webview подменяет custom protocol на Windows/Android.
const WEBVIEW_HOSTS: [&str; 2] = ["dioxus.localhost", "tauri.localhost"];

/// Токен и списки разрешённых `Host`/`Origin`.
#[derive(Debug, Clone)]
pub struct Access {
    /// Токен для `Authorization: Bearer`.
    pub token: String,
    /// Дополнительные хосты (без порта, без учёта регистра), например имя VDS.
    pub allowed_hosts: Vec<String>,
    /// Дополнительные `Origin`: точное значение (`https://focus.example.com`)
    /// или префикс-схема с `://` на конце (`myapp://`).
    pub allowed_origins: Vec<String>,
}

impl Access {
    /// Только токен, списки — встроенные.
    pub fn new(token: impl Into<String>) -> Self {
        Self {
            token: token.into(),
            allowed_hosts: Vec::new(),
            allowed_origins: Vec::new(),
        }
    }

    /// Разрешён ли `Host` (с портом или без).
    pub fn host_allowed(&self, host: &str) -> bool {
        let name = strip_port(host).to_ascii_lowercase();
        LOOPBACK_HOSTS.contains(&name.as_str())
            || self
                .allowed_hosts
                .iter()
                .any(|h| h.eq_ignore_ascii_case(&name))
    }

    /// Разрешён ли `Origin`.
    pub fn origin_allowed(&self, origin: &str) -> bool {
        let o = origin.trim().to_ascii_lowercase();
        if o == "null" || WEBVIEW_SCHEMES.iter().any(|s| o.starts_with(s)) {
            return true;
        }
        if let Some(host) = o
            .strip_prefix("http://")
            .or_else(|| o.strip_prefix("https://"))
            && !host.contains('/')
        {
            let name = strip_port(host);
            if LOOPBACK_HOSTS.contains(&name) || WEBVIEW_HOSTS.contains(&name) {
                return true;
            }
        }
        self.allowed_origins.iter().any(|a| {
            let a = a.trim().to_ascii_lowercase();
            if a.ends_with("://") {
                o.starts_with(&a)
            } else {
                o == a.trim_end_matches('/')
            }
        })
    }

    /// Совпадает ли предъявленный токен (сравнение за постоянное время).
    pub fn token_matches(&self, presented: &str) -> bool {
        let a = self.token.as_bytes();
        let b = presented.as_bytes();
        a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
    }
}

/// `host[:port]` → `host`; IPv6 — в скобках, как в заголовке `Host`.
fn strip_port(host: &str) -> &str {
    if host.starts_with('[') {
        return host.find(']').map_or(host, |i| &host[..=i]);
    }
    host.rsplit_once(':').map_or(host, |(h, _)| h)
}

fn forbidden(msg: impl Into<String>) -> Response {
    RestError(ApiError::Forbidden(msg.into())).into_response()
}

/// Middleware проверок (см. описание модуля).
pub async fn guard(State(access): State<Arc<Access>>, req: Request, next: Next) -> Response {
    let headers = req.headers();

    let host = headers
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned)
        .or_else(|| req.uri().authority().map(|a| a.as_str().to_owned()));
    match host {
        Some(h) if access.host_allowed(&h) => {}
        Some(h) => {
            tracing::warn!(host = %h, "отклонён запрос: чужой Host");
            return forbidden(format!("Host «{h}» не разрешён"));
        }
        None => return forbidden("нет заголовка Host"),
    }

    let origin = match headers.get(header::ORIGIN) {
        None => None,
        Some(v) => match v.to_str() {
            Ok(o) if access.origin_allowed(o) => Some(v.clone()),
            Ok(o) => {
                tracing::warn!(origin = %o, "отклонён запрос: чужой Origin");
                return forbidden(format!("Origin «{o}» не разрешён"));
            }
            Err(_) => return forbidden("неверный заголовок Origin"),
        },
    };

    if req.method() == Method::OPTIONS && origin.is_some() {
        let mut resp = StatusCode::NO_CONTENT.into_response();
        add_cors(resp.headers_mut(), origin, true);
        return resp;
    }

    let public = req.method() == Method::GET && req.uri().path() == "/api/health";
    if !public && !authorized(&access, &req) {
        let mut resp = RestError(ApiError::Unauthorized).into_response();
        resp.headers_mut().insert(
            header::WWW_AUTHENTICATE,
            HeaderValue::from_static("Bearer realm=\"focus-flow\""),
        );
        add_cors(resp.headers_mut(), origin, false);
        return resp;
    }

    let mut resp = next.run(req).await;
    add_cors(resp.headers_mut(), origin, false);
    resp
}

fn authorized(access: &Access, req: &Request) -> bool {
    let bearer = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(str::trim);
    if let Some(t) = bearer {
        return access.token_matches(t);
    }
    if req.method() == Method::GET && req.uri().path() == "/api/events" {
        let query = req.uri().query().unwrap_or("");
        return query
            .split('&')
            .filter_map(|kv| kv.strip_prefix("token="))
            .any(|t| access.token_matches(t));
    }
    false
}

fn add_cors(h: &mut HeaderMap, origin: Option<HeaderValue>, preflight: bool) {
    let Some(origin) = origin else { return };
    h.insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, origin);
    h.append(header::VARY, HeaderValue::from_static("Origin"));
    if preflight {
        h.insert(
            header::ACCESS_CONTROL_ALLOW_METHODS,
            HeaderValue::from_static("GET, POST, PUT, PATCH, OPTIONS"),
        );
        h.insert(
            header::ACCESS_CONTROL_ALLOW_HEADERS,
            HeaderValue::from_static("authorization, content-type"),
        );
        h.insert(
            header::ACCESS_CONTROL_MAX_AGE,
            HeaderValue::from_static("600"),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hosts() {
        let mut a = Access::new("t");
        for ok in [
            "127.0.0.1",
            "127.0.0.1:7878",
            "localhost:7878",
            "LOCALHOST",
            "[::1]:7878",
        ] {
            assert!(a.host_allowed(ok), "{ok}");
        }
        for bad in [
            "evil.example",
            "evil.example:7878",
            "127.0.0.1.evil.example",
            "0.0.0.0:7878",
        ] {
            assert!(!a.host_allowed(bad), "{bad}");
        }
        a.allowed_hosts.push("focus.example.com".into());
        assert!(a.host_allowed("Focus.Example.com:443"));
    }

    #[test]
    fn origins() {
        let mut a = Access::new("t");
        for ok in [
            "dioxus://index.html",
            "tauri://localhost",
            "http://dioxus.localhost",
            "https://dioxus.localhost",
            "http://127.0.0.1:8080",
            "http://localhost:8080",
            "null",
        ] {
            assert!(a.origin_allowed(ok), "{ok}");
        }
        for bad in [
            "https://evil.example",
            "http://localhost.evil.example",
            "http://127.0.0.1.evil.example:7878",
            "myapp://x",
        ] {
            assert!(!a.origin_allowed(bad), "{bad}");
        }
        a.allowed_origins = vec!["https://focus.example.com/".into(), "myapp://".into()];
        assert!(a.origin_allowed("https://focus.example.com"));
        assert!(a.origin_allowed("myapp://x"));
        assert!(!a.origin_allowed("https://focus.example.com.evil"));
    }

    #[test]
    fn token() {
        let a = Access::new("secret");
        assert!(a.token_matches("secret"));
        assert!(!a.token_matches("secreT"));
        assert!(!a.token_matches("secret2"));
        assert!(!a.token_matches(""));
    }
}
