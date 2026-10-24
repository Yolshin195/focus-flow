//! REST через `tower::ServiceExt::oneshot`: доступ (токен, Host, Origin),
//! каждый путь SPEC, коды ошибок, SSE.

mod common;

use std::time::Duration as StdDuration;

use axum::body::Body;
use axum::http::{Method, Request, StatusCode, header};
use chrono::Duration;
use common::{Env, TOKEN, build};
use http_body_util::BodyExt as _;
use serde_json::{Value, json};

// ------------------------------------------------------------------ доступ

#[tokio::test]
async fn health_is_public() {
    let env = Env::new().await;
    let resp = env
        .send(build(Method::GET, "/api/health", None, None))
        .await;
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn token_required() {
    let env = Env::new().await;
    for token in [None, Some("wrong")] {
        let resp = env
            .send(build(Method::GET, "/api/status", None, token))
            .await;
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED, "{token:?}");
        assert!(resp.headers().contains_key(header::WWW_AUTHENTICATE));
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let v: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v, json!({"code": "unauthorized"}));
    }
    let resp = env
        .send(build(Method::GET, "/api/status", None, Some(TOKEN)))
        .await;
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn query_token_only_for_events() {
    let env = Env::new().await;
    let resp = env
        .send(build(
            Method::GET,
            &format!("/api/status?token={TOKEN}"),
            None,
            None,
        ))
        .await;
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    let resp = env
        .send(build(Method::GET, "/api/events?token=wrong", None, None))
        .await;
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    let resp = env
        .send(build(
            Method::GET,
            &format!("/api/events?token={TOKEN}"),
            None,
            None,
        ))
        .await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(resp.headers()[header::CONTENT_TYPE], "text/event-stream");
}

#[tokio::test]
async fn foreign_host_rejected() {
    let env = Env::new().await;
    for host in ["evil.example", "evil.example:7878", "127.0.0.1.nip.io:7878"] {
        let req = Request::get("/api/health")
            .header(header::HOST, host)
            .header(header::AUTHORIZATION, format!("Bearer {TOKEN}"))
            .body(Body::empty())
            .unwrap();
        let resp = env.send(req).await;
        assert_eq!(resp.status(), StatusCode::FORBIDDEN, "{host}");
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let v: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v["code"], "forbidden");
    }
    let req = Request::get("/api/health")
        .header(header::HOST, "localhost:7878")
        .body(Body::empty())
        .unwrap();
    assert_eq!(env.send(req).await.status(), StatusCode::OK);
}

#[tokio::test]
async fn origin_check_and_cors() {
    let env = Env::new().await;
    let with_origin = |origin: &str| {
        let mut r = build(Method::GET, "/api/status", None, Some(TOKEN));
        r.headers_mut()
            .insert(header::ORIGIN, origin.parse().unwrap());
        r
    };
    let resp = env.send(with_origin("https://evil.example")).await;
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);

    for origin in ["dioxus://index.html", "http://dioxus.localhost", "null"] {
        let resp = env.send(with_origin(origin)).await;
        assert_eq!(resp.status(), StatusCode::OK, "{origin}");
        assert_eq!(resp.headers()[header::ACCESS_CONTROL_ALLOW_ORIGIN], origin);
    }

    // Preflight без токена.
    let req = Request::builder()
        .method(Method::OPTIONS)
        .uri("/api/sessions/work")
        .header(header::HOST, "127.0.0.1:7878")
        .header(header::ORIGIN, "http://localhost:8080")
        .header(header::ACCESS_CONTROL_REQUEST_METHOD, "POST")
        .body(Body::empty())
        .unwrap();
    let resp = env.send(req).await;
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    assert!(
        resp.headers()[header::ACCESS_CONTROL_ALLOW_HEADERS]
            .to_str()
            .unwrap()
            .contains("authorization")
    );

    // Без Origin (desktop через нативный HTTP-клиент, curl) — без CORS.
    let resp = env
        .send(build(Method::GET, "/api/status", None, Some(TOKEN)))
        .await;
    assert!(
        !resp
            .headers()
            .contains_key(header::ACCESS_CONTROL_ALLOW_ORIGIN)
    );
}

// ------------------------------------------------------------------ пути

#[tokio::test]
async fn day_and_status() {
    let env = Env::new().await;
    let (s, v) = env.call(Method::GET, "/api/status", None).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(v["day"], Value::Null);
    assert_eq!(v["server_time"], "2026-10-24T09:00:00Z");

    // Закрыть неоткрытый день → 422 validation.
    let (s, v) = env.call(Method::POST, "/api/day/close", None).await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(v["code"], "validation");

    let (s, v) = env.call(Method::POST, "/api/day/open", None).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(v["day"]["date"], "2026-10-24");
    let (s, v) = env.call(Method::POST, "/api/day/close", None).await;
    assert_eq!(s, StatusCode::OK);
    assert!(v["day"]["closed_at"].is_string());

    let (s, v) = env.call(Method::GET, "/api/days/2026-10-24", None).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(v["date"], "2026-10-24");
    assert!(v["totals"].is_object());

    let (s, v) = env.call(Method::GET, "/api/days/24-10-2026", None).await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(v["code"], "validation");
}

#[tokio::test]
async fn tasks_crud() {
    let env = Env::new().await;
    let (s, parent) = env
        .call(Method::POST, "/api/tasks", Some(json!({"title": "задача"})))
        .await;
    assert_eq!(s, StatusCode::CREATED);
    let (s, item) = env
        .call(
            Method::POST,
            "/api/tasks",
            Some(json!({"title": "пункт", "parent_id": parent["id"]})),
        )
        .await;
    assert_eq!(s, StatusCode::CREATED);
    assert_eq!(item["parent_id"], parent["id"]);

    let (s, tree) = env.call(Method::GET, "/api/tasks", None).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(tree[0]["children"][0]["id"], item["id"]);

    let uri = format!("/api/tasks/{}", item["id"].as_str().unwrap());
    let (s, v) = env
        .call(
            Method::PATCH,
            &uri,
            Some(json!({"title": "новое", "planned_for": "2026-10-25", "notes": null})),
        )
        .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(v["title"], "новое");
    assert_eq!(v["planned_for"], "2026-10-25");

    // Ошибки: пустое название, кривой JSON, неизвестный id, не UUID.
    let (s, v) = env
        .call(Method::POST, "/api/tasks", Some(json!({"title": "  "})))
        .await;
    assert_eq!(
        (s, v["code"].as_str()),
        (StatusCode::UNPROCESSABLE_ENTITY, Some("validation"))
    );
    let resp = env
        .send(build(
            Method::POST,
            "/api/tasks",
            Some("{".into()),
            Some(TOKEN),
        ))
        .await;
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let (s, v) = env
        .call(
            Method::PATCH,
            "/api/tasks/00000000-0000-0000-0000-000000000001",
            Some(json!({"title": "x"})),
        )
        .await;
    assert_eq!(
        (s, v["code"].as_str()),
        (StatusCode::NOT_FOUND, Some("item_not_found"))
    );
    let (s, _) = env
        .call(Method::PATCH, "/api/tasks/nope", Some(json!({})))
        .await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn sessions_flow() {
    let env = Env::new().await;
    let (_, task) = env
        .call(Method::POST, "/api/tasks", Some(json!({"title": "задача"})))
        .await;
    let (_, item) = env
        .call(
            Method::POST,
            "/api/tasks",
            Some(json!({"title": "пункт", "parent_id": task["id"]})),
        )
        .await;

    // Задача с пунктами → 422 item_required.
    let (s, v) = env
        .call(
            Method::POST,
            "/api/sessions/work",
            Some(json!({"item_id": task["id"]})),
        )
        .await;
    assert_eq!(
        (s, v["code"].as_str()),
        (StatusCode::UNPROCESSABLE_ENTITY, Some("item_required"))
    );

    let (s, v) = env
        .call(
            Method::POST,
            "/api/sessions/work",
            Some(json!({"item_id": item["id"]})),
        )
        .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(v["active"]["kind"], "work");
    assert_eq!(v["active"]["ends_at"], "2026-10-24T09:50:00Z");

    // Повторный старт → 409; «Стоп» во время работы → 409 wrong_session_kind.
    let (s, v) = env.call(Method::POST, "/api/sessions/planning", None).await;
    assert_eq!(
        (s, v["code"].as_str()),
        (StatusCode::CONFLICT, Some("session_already_active"))
    );
    let (s, v) = env
        .call(Method::POST, "/api/sessions/active/stop", None)
        .await;
    assert_eq!(s, StatusCode::CONFLICT);
    assert_eq!(v["code"], "wrong_session_kind");
    assert_eq!(
        v["details"],
        json!({"expected": "planning", "actual": "work"})
    );

    env.clock.advance(Duration::minutes(20));
    let (s, v) = env
        .call(
            Method::POST,
            "/api/sessions/active/done",
            Some(json!({"next": "planning"})),
        )
        .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(v["ended"]["end_reason"], "task_done");
    assert_eq!(v["ended"]["duration_sec"], 20 * 60);
    assert_eq!(v["status"]["active"]["kind"], "planning");

    env.clock.advance(Duration::minutes(5));
    let (s, v) = env
        .call(Method::POST, "/api/sessions/active/stop", None)
        .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(v["ended"]["end_reason"], "stopped");
    assert_eq!(v["status"]["worked_today_sec"], 25 * 60);

    let (s, v) = env
        .call(Method::POST, "/api/sessions/active/interrupt", None)
        .await;
    assert_eq!(
        (s, v["code"].as_str()),
        (StatusCode::CONFLICT, Some("no_active_session"))
    );

    let (_, other) = env
        .call(Method::POST, "/api/tasks", Some(json!({"title": "второй"})))
        .await;
    env.call(
        Method::POST,
        "/api/sessions/work",
        Some(json!({"item_id": other["id"]})),
    )
    .await;
    let (s, v) = env
        .call(Method::POST, "/api/sessions/active/interrupt", None)
        .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(v["ended"]["end_reason"], "interrupted");

    let (s, v) = env.call(Method::POST, "/api/sessions/rest", None).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(v["active"]["kind"], "rest");

    // Неверное тело done.
    let (s, _) = env
        .call(
            Method::POST,
            "/api/sessions/active/done",
            Some(json!({"next": "nap"})),
        )
        .await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn import_settings_plugins() {
    let env = Env::new().await;
    let (s, v) = env
        .call(Method::POST, "/api/import", Some(json!({"url": "PROJ-1"})))
        .await;
    assert_eq!(
        (s, v["code"].as_str()),
        (StatusCode::SERVICE_UNAVAILABLE, Some("plugin_unavailable"))
    );

    let (s, mut settings) = env.call(Method::GET, "/api/settings", None).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(settings["work_min"], 50);
    settings["work_min"] = json!(25);
    let (s, v) = env
        .call(Method::PUT, "/api/settings", Some(settings.clone()))
        .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(v["work_min"], 25);
    settings["rest_min"] = json!(0);
    let (s, v) = env.call(Method::PUT, "/api/settings", Some(settings)).await;
    assert_eq!(
        (s, v["code"].as_str()),
        (StatusCode::UNPROCESSABLE_ENTITY, Some("validation"))
    );

    let (s, v) = env.call(Method::GET, "/api/plugins/status", None).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(v, json!([]));
}

// ------------------------------------------------------------------ SSE

/// Читает из тела SSE, пока не встретится событие `name`; возвращает его `data`.
async fn read_event(body: &mut Body, buf: &mut String, name: &str) -> Value {
    let deadline = tokio::time::Instant::now() + StdDuration::from_secs(5);
    loop {
        while let Some(end) = buf.find("\n\n") {
            let block: String = buf.drain(..end + 2).collect();
            let mut ev = None;
            let mut data = String::new();
            for line in block.lines() {
                if let Some(v) = line.strip_prefix("event: ") {
                    ev = Some(v.to_owned());
                } else if let Some(v) = line.strip_prefix("data: ") {
                    data.push_str(v);
                }
            }
            if ev.as_deref() == Some(name) {
                return serde_json::from_str(&data).unwrap();
            }
        }
        let frame = tokio::time::timeout_at(deadline, body.frame())
            .await
            .unwrap_or_else(|_| panic!("SSE: нет события {name} за 5 с; буфер: {buf:?}"))
            .expect("SSE: поток закончился")
            .unwrap();
        if let Ok(bytes) = frame.into_data() {
            buf.push_str(&String::from_utf8_lossy(&bytes));
        }
    }
}

#[tokio::test]
async fn sse_emits_status_changed_after_command() {
    let env = Env::new().await;
    let resp = env
        .send(build(Method::GET, "/api/events", None, Some(TOKEN)))
        .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let mut body = resp.into_body();
    let mut buf = String::new();

    // Первым — текущий статус.
    let first = read_event(&mut body, &mut buf, "status_changed").await;
    assert_eq!(first["type"], "status_changed");
    assert_eq!(first["data"]["active"], Value::Null);

    env.call(Method::POST, "/api/sessions/planning", None).await;
    let ev = read_event(&mut body, &mut buf, "status_changed").await;
    assert_eq!(ev["data"]["active"]["kind"], "planning");

    env.call(Method::POST, "/api/sessions/active/stop", None)
        .await;
    let ev = read_event(&mut body, &mut buf, "session_ended").await;
    assert_eq!(ev["data"]["session"]["end_reason"], "stopped");

    // Остановка сервиса закрывает поток.
    env.state.shutdown.cancel();
    let end = tokio::time::timeout(StdDuration::from_secs(5), async {
        while let Some(f) = body.frame().await {
            f.unwrap();
        }
    })
    .await;
    assert!(end.is_ok(), "SSE-поток не закрылся при остановке");
}
