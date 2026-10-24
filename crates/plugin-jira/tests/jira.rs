//! Плагин Jira против заглушки `wiremock`: автопоиск поля, импорт,
//! добавление и завершение пунктов (с перечитыванием поля), worklog,
//! авторизация Data Center/Cloud, ненастроенный плагин, маршруты.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use chrono::{Duration, TimeZone, Utc};
use focus_api_types::{NewTask, SessionId, SessionKind, TaskPatch, TaskStatus};
use focus_core::{ManualClock, Session};
use focus_plugin_api::{DomainEvent, Plugin, PluginCtx, TaskSource as _};
use focus_plugin_jira::JiraPlugin;
use focus_server::{App, Config, Registration, router};
use focus_storage::Storage;
use http_body_util::BodyExt as _;
use serde_json::{Value, json};
use tower::ServiceExt as _;
use wiremock::matchers::{body_json, header as h, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

const TOKEN: &str = "t";
const FIELD: &str = "customfield_10100";
const SAMPLE: &str = "- Вердикт в PDF и в чате считается по разным правилам\n- (1) Название отчёта по тестированию — с временем\n+ (30) Пароль не выводить в историю\n~ (31) Останавливать прогон при сбое\nx Одна задача за запуск\n## Заметки\n- Подключить роутер llm ноду";

struct Env {
    app: App,
    plugin: JiraPlugin,
    clock: Arc<ManualClock>,
    jira: MockServer,
    _dir: tempfile::TempDir,
}

async fn env() -> Env {
    let clock = Arc::new(ManualClock::new(
        Utc.with_ymd_and_hms(2026, 10, 24, 9, 0, 0).unwrap(),
    ));
    let dir = tempfile::tempdir().unwrap();
    let plugin = JiraPlugin::new();
    let app = App::from_parts(
        Config::with_data_dir(dir.path()),
        Storage::open_in_memory().await.unwrap(),
        clock.clone(),
        vec![Registration::Source(Arc::new(plugin.clone()))],
        TOKEN,
    )
    .await
    .unwrap();
    Env {
        app,
        plugin,
        clock,
        jira: MockServer::start().await,
        _dir: dir,
    }
}

impl Env {
    async fn http(&self, m: &str, uri: &str, body: Option<Value>) -> (StatusCode, Value) {
        let b = Request::builder()
            .method(m)
            .uri(uri)
            .header(header::HOST, "127.0.0.1:7878")
            .header(header::AUTHORIZATION, format!("Bearer {TOKEN}"));
        let req = match body {
            Some(v) => b
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(v.to_string()))
                .unwrap(),
            None => b.body(Body::empty()).unwrap(),
        };
        let resp = router(&self.app).oneshot(req).await.unwrap();
        let status = resp.status();
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    }

    /// Настроить через `PUT /api/plugins/jira/settings`.
    async fn configure(&self, extra: Value) -> Value {
        let mut body = json!({"base_url": self.jira.uri(), "token": "pat-secret"});
        for (k, v) in extra.as_object().unwrap() {
            body[k] = v.clone();
        }
        let (code, v) = self
            .http("PUT", "/api/plugins/jira/settings", Some(body))
            .await;
        assert_eq!(code, StatusCode::OK, "{v}");
        v
    }

    async fn deliver(&self) -> usize {
        self.app.plugins.deliver_due().await.unwrap()
    }

    fn ctx(&self) -> PluginCtx {
        PluginCtx::new(
            "jira",
            self.app.plugins.service_slot().clone(),
            self.app.storage.pool().clone(),
            &self.app.config.secrets_dir(),
        )
    }

    async fn mount_issue(&self, key: &str, checklist: &str) {
        Mock::given(method("GET"))
            .and(path(format!("/rest/api/2/issue/{key}")))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "key": key,
                "fields": {"summary": "Отчёт по тестированию", FIELD: checklist}
            })))
            .mount(&self.jira)
            .await;
    }

    async fn mount_put(&self, key: &str) {
        Mock::given(method("PUT"))
            .and(path(format!("/rest/api/2/issue/{key}")))
            .respond_with(ResponseTemplate::new(204))
            .mount(&self.jira)
            .await;
    }

    async fn requests(&self, m: &str, p: &str) -> Vec<wiremock::Request> {
        self.jira
            .received_requests()
            .await
            .unwrap()
            .into_iter()
            .filter(|r| r.method.as_str() == m && r.url.path() == p)
            .collect()
    }

    /// Импорт `ABC-1` с чек-листом `SAMPLE` (поле задано в настройках) и
    /// доставка `TaskImported`.
    async fn imported(&self) -> focus_api_types::ImportResult {
        self.configure(json!({"checklist_field_id": FIELD})).await;
        self.mount_issue("ABC-1", SAMPLE).await;
        let r = self
            .app
            .service
            .import(&format!("{}/browse/ABC-1", self.jira.uri()))
            .await
            .unwrap();
        self.deliver().await;
        self.jira.reset().await;
        r
    }
}

fn put_text(r: &wiremock::Request) -> String {
    let v: Value = serde_json::from_slice(&r.body).unwrap();
    v["fields"][FIELD].as_str().unwrap().to_owned()
}

#[tokio::test]
async fn unconfigured_plugin_is_enabled_and_harmless() {
    let e = env().await;
    let st = e.app.service.plugin_status().await.unwrap();
    assert_eq!(st[0].id, "jira");
    assert!(st[0].enabled && st[0].healthy, "{st:?}");
    assert_eq!(
        st[0].last_error.as_deref(),
        Some("не настроен: не задан base_url")
    );
    // События идут и закрываются без сети.
    let t = e
        .app
        .service
        .create_task(NewTask::new("задача"))
        .await
        .unwrap();
    let i = e
        .app
        .service
        .create_task(NewTask::child(t.id, "пункт"))
        .await
        .unwrap();
    e.app.service.start_work(i.id).await.unwrap();
    e.clock.advance(Duration::minutes(5));
    e.app.service.interrupt().await.unwrap();
    assert!(e.deliver().await >= 3);
    let st = e.app.service.plugin_status().await.unwrap();
    assert!(st[0].healthy);
    assert_eq!(st[0].outbox_pending, 0);
    // Импорт: источник ещё не подходит ни к чему.
    let err = e.app.service.import("ABC-1").await.unwrap_err();
    assert!(
        matches!(err, focus_core::CoreError::PluginUnavailable(_)),
        "{err:?}"
    );
    // Выключить можно явно.
    let (code, v) = e
        .http(
            "PUT",
            "/api/plugins/jira/settings",
            Some(json!({"enabled": false})),
        )
        .await;
    assert_eq!(code, StatusCode::OK, "{v}");
    assert!(!e.app.plugins.is_enabled("jira"));
    let (code, _) = e.http("GET", "/api/plugins/jira/settings", None).await;
    assert_eq!(code, StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
async fn settings_routes_hide_token_and_test_endpoint() {
    let e = env().await;
    let (code, v) = e.http("GET", "/api/plugins/jira/settings", None).await;
    assert_eq!(code, StatusCode::OK);
    assert_eq!(v["configured"], false);
    assert_eq!(v["has_token"], false);

    let v = e.configure(json!({"mode": "data_center"})).await;
    assert_eq!(v["configured"], true, "{v}");
    assert_eq!(v["has_token"], true);
    assert!(!v.to_string().contains("pat-secret"));
    let (_, v) = e.http("GET", "/api/plugins/jira/settings", None).await;
    assert!(!v.to_string().contains("pat-secret"), "{v}");
    assert_eq!(v["base_url"], e.jira.uri());
    // В общих настройках токена тоже нет.
    let s = e.app.service.settings().await.unwrap();
    assert!(!serde_json::to_string(&s).unwrap().contains("pat-secret"));
    let secret = e.app.config.secrets_dir().join("jira").join("token");
    assert_eq!(std::fs::read_to_string(secret).unwrap(), "pat-secret");

    // Неверные значения — 422.
    for bad in [
        json!({"base_url": "jira.example.com"}),
        json!({"mode": "server"}),
        json!({"token": 5}),
        json!({"password": "x"}),
    ] {
        let (code, v) = e
            .http("PUT", "/api/plugins/jira/settings", Some(bad.clone()))
            .await;
        assert_eq!(code, StatusCode::UNPROCESSABLE_ENTITY, "{bad} → {v}");
    }

    // test: успех и 401.
    Mock::given(method("GET"))
        .and(path("/rest/api/2/myself"))
        .and(h("authorization", "Bearer pat-secret"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"displayName": "Алексей"})))
        .mount(&e.jira)
        .await;
    let (code, v) = e.http("POST", "/api/plugins/jira/test", None).await;
    assert_eq!(code, StatusCode::OK, "{v}");
    assert_eq!(v["user"], "Алексей");

    e.configure(json!({"token": "wrong"})).await;
    Mock::given(method("GET"))
        .and(path("/rest/api/2/myself"))
        .respond_with(ResponseTemplate::new(401))
        .mount(&e.jira)
        .await;
    let (code, v) = e.http("POST", "/api/plugins/jira/test", None).await;
    assert_eq!(code, StatusCode::SERVICE_UNAVAILABLE, "{v}");
    let msg = v["details"].as_str().unwrap();
    assert!(
        msg.contains("401") && msg.contains("неверный токен"),
        "{msg}"
    );
    let (_, v) = e.http("GET", "/api/plugins/jira/settings", None).await;
    assert!(v["last_error"].as_str().unwrap().contains("401"));

    // Токен удаляется пустой строкой.
    let (_, v) = e
        .http(
            "PUT",
            "/api/plugins/jira/settings",
            Some(json!({"token": ""})),
        )
        .await;
    assert_eq!(v["has_token"], false);
    assert_eq!(v["problem"], "нет токена");
}

#[tokio::test]
async fn cloud_uses_basic_auth() {
    let e = env().await;
    e.configure(json!({"mode": "cloud", "token": "tok-cloud"}))
        .await;
    let (code, v) = e.http("POST", "/api/plugins/jira/test", None).await;
    assert_eq!(code, StatusCode::UNPROCESSABLE_ENTITY, "{v}");
    assert!(v["details"].as_str().unwrap().contains("email"));

    e.configure(json!({"mode": "cloud", "token": "tok-cloud", "email": "me@example.com"}))
        .await;
    Mock::given(method("GET"))
        .and(path("/rest/api/2/myself"))
        .and(h("authorization", "Basic bWVAZXhhbXBsZS5jb206dG9rLWNsb3Vk"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"displayName": "Cloud"})))
        .mount(&e.jira)
        .await;
    let (code, v) = e.http("POST", "/api/plugins/jira/test", None).await;
    assert_eq!(code, StatusCode::OK, "{v}");
    assert_eq!(v["user"], "Cloud");
}

#[tokio::test]
async fn import_with_field_autodiscovery() {
    let e = env().await;
    e.configure(json!({})).await;
    Mock::given(method("GET"))
        .and(path("/rest/api/2/field"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
            {"id": "summary", "name": "Summary"},
            {"id": "customfield_10200", "name": "Smart Checklist Progress"},
            {"id": FIELD, "name": "Smart Checklist"},
        ])))
        .expect(1)
        .mount(&e.jira)
        .await;
    Mock::given(method("GET"))
        .and(path("/rest/api/2/issue/ABC-1"))
        .and(query_param("fields", format!("summary,{FIELD}")))
        .and(h("authorization", "Bearer pat-secret"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "key": "ABC-1",
            "fields": {"summary": "Отчёт по тестированию", FIELD: SAMPLE}
        })))
        .mount(&e.jira)
        .await;
    // Чужой хост и неверная ссылка не подходят.
    assert!(!e.plugin.matches("https://other.example.com/browse/ABC-1"));
    assert!(e.plugin.matches("ABC-1"));

    let url = format!("{}/browse/ABC-1", e.jira.uri());
    let r = e.app.service.import(&url).await.unwrap();
    assert!(r.created);
    assert_eq!(r.source, "jira");
    assert_eq!(r.task.title, "ABC-1 · Отчёт по тестированию");
    assert_eq!(r.task.external_url.as_deref(), Some(url.as_str()));
    let items: Vec<(&str, TaskStatus)> = r
        .task
        .children
        .iter()
        .map(|c| (c.title.as_str(), c.status))
        .collect();
    assert_eq!(
        items,
        [
            (
                "Вердикт в PDF и в чате считается по разным правилам",
                TaskStatus::Planned
            ),
            (
                "(1) Название отчёта по тестированию — с временем",
                TaskStatus::Planned
            ),
            ("(30) Пароль не выводить в историю", TaskStatus::Done),
            ("(31) Останавливать прогон при сбое", TaskStatus::InProgress),
            ("Одна задача за запуск", TaskStatus::Cancelled),
            ("Подключить роутер llm ноду", TaskStatus::Planned),
        ]
    );
    // Повторный импорт по ключу: та же задача, поле — из кэша, без дублей.
    let r = e.app.service.import("ABC-1").await.unwrap();
    assert!(!r.created);
    assert_eq!(r.added_items, 0);
    assert_eq!(r.task.children.len(), 6);
    let (_, v) = e.http("GET", "/api/plugins/jira/settings", None).await;
    assert_eq!(v["checklist_field_resolved"], FIELD);

    // Новая строка в Jira — догружается одна.
    e.jira.reset().await;
    e.mount_issue("ABC-1", &format!("{SAMPLE}\n- Новая строка"))
        .await;
    let r = e.app.service.import("ABC-1").await.unwrap();
    assert_eq!(r.added_items, 1);
    assert_eq!(r.task.children.len(), 7);

    // Нет такой задачи — validation.
    e.jira.reset().await;
    let err = e.app.service.import("ABC-404").await.unwrap_err();
    assert!(
        matches!(err, focus_core::CoreError::Validation(_)),
        "{err:?}"
    );
}

#[tokio::test]
async fn new_item_appends_line() {
    let e = env().await;
    let r = e.imported().await;
    e.mount_issue("ABC-1", SAMPLE).await;
    e.mount_put("ABC-1").await;
    e.app
        .service
        .create_task(NewTask::child(r.task.id, "Новый пункт"))
        .await
        .unwrap();
    assert_eq!(e.deliver().await, 1);
    let puts = e.requests("PUT", "/rest/api/2/issue/ABC-1").await;
    assert_eq!(puts.len(), 1);
    assert_eq!(put_text(&puts[0]), format!("{SAMPLE}\n- Новый пункт"));
    let n: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM jira_items WHERE line_text = 'Новый пункт' AND issue_key = 'ABC-1'",
    )
    .fetch_one(e.app.storage.pool())
    .await
    .unwrap();
    assert_eq!(n, 1);
    // Пункт без связанной задачи — без сети.
    e.jira.reset().await;
    let t = e
        .app
        .service
        .create_task(NewTask::new("своя"))
        .await
        .unwrap();
    e.app
        .service
        .create_task(NewTask::child(t.id, "x"))
        .await
        .unwrap();
    e.deliver().await;
    assert!(e.jira.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn completed_item_rereads_and_keeps_remote_edits() {
    let e = env().await;
    let r = e.imported().await;
    let item = r
        .task
        .children
        .iter()
        .find(|c| c.title.starts_with("(1)"))
        .unwrap();
    // Пока мы работали, в Jira поменяли другую строку и добавили новую.
    let remote = SAMPLE.replace("- Вердикт в PDF", "+ Вердикт в PDF") + "\n- Строка из Jira";
    e.mount_issue("ABC-1", &remote).await;
    e.mount_put("ABC-1").await;
    e.app
        .service
        .update_task(
            item.id,
            TaskPatch {
                status: Some(TaskStatus::Done),
                ..TaskPatch::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(e.deliver().await, 1);
    let puts = e.requests("PUT", "/rest/api/2/issue/ABC-1").await;
    assert_eq!(puts.len(), 1);
    let want = remote.replace("- (1) Название отчёта", "+ (1) Название отчёта");
    assert_eq!(put_text(&puts[0]), want);

    // Строку удалили в Jira — предупреждение и пропуск, без повторов.
    e.jira.reset().await;
    e.mount_issue("ABC-1", "- другое").await;
    e.mount_put("ABC-1").await;
    let other = r
        .task
        .children
        .iter()
        .find(|c| c.title.starts_with("Подключить"))
        .unwrap();
    e.app
        .service
        .update_task(
            other.id,
            TaskPatch {
                status: Some(TaskStatus::Done),
                ..TaskPatch::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(e.deliver().await, 1);
    assert!(
        e.requests("PUT", "/rest/api/2/issue/ABC-1")
            .await
            .is_empty()
    );
    let st = e.app.service.plugin_status().await.unwrap();
    assert!(st[0].healthy && st[0].outbox_pending == 0, "{st:?}");
}

#[tokio::test]
async fn worklog_payload_and_idempotency() {
    let e = env().await;
    let r = e.imported().await;
    let item = r
        .task
        .children
        .iter()
        .find(|c| c.title.starts_with("Вердикт"))
        .unwrap();
    Mock::given(method("POST"))
        .and(path("/rest/api/2/issue/ABC-1/worklog"))
        .and(body_json(json!({
            "started": "2026-10-24T09:00:00.000+0000",
            "timeSpentSeconds": 1200,
            "comment": item.title,
        })))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({"id": "10042"})))
        .expect(1)
        .mount(&e.jira)
        .await;
    e.app.service.start_work(item.id).await.unwrap();
    e.clock.advance(Duration::minutes(20));
    let ended = e.app.service.interrupt().await.unwrap().ended;
    e.deliver().await;
    assert_eq!(
        e.requests("POST", "/rest/api/2/issue/ABC-1/worklog")
            .await
            .len(),
        1
    );
    let row: (String, String) =
        sqlx::query_as("SELECT worklog_id, status FROM jira_worklogs WHERE session_id = ?")
            .bind(ended.id.to_string())
            .fetch_one(e.app.storage.pool())
            .await
            .unwrap();
    assert_eq!(row, ("10042".into(), "sent".into()));

    // Повторная доставка того же события — без второго worklog.
    let tree = e.app.service.list_tasks().await.unwrap();
    let parent = tree.iter().find(|t| t.id == r.task.id).unwrap();
    let to_task = |d: &focus_api_types::TaskDto| focus_core::Task {
        id: d.id,
        parent_id: d.parent_id,
        title: d.title.clone(),
        notes: d.notes.clone(),
        status: d.status,
        planned_for: d.planned_for,
        estimate_intervals: d.estimate_intervals,
        sort_order: d.sort_order,
        created_at: d.created_at,
        done_at: d.done_at,
        external_url: d.external_url.clone(),
    };
    let ev = DomainEvent::SessionClosed {
        session: Session {
            id: ended.id,
            day_id: ended.day_id,
            kind: SessionKind::Work,
            task_id: Some(item.id),
            started_at: ended.started_at,
            ended_at: ended.ended_at,
            limit_sec: ended.limit_sec,
            end_reason: ended.end_reason,
        },
        item: Some(to_task(item)),
        parent: Some(to_task(parent)),
    };
    e.plugin.on_event(&ev, &e.ctx()).await.unwrap();
    assert_eq!(
        e.requests("POST", "/rest/api/2/issue/ABC-1/worklog")
            .await
            .len(),
        1
    );

    // Короткая сессия — минимум 60 с; прерванные не списываются, если выключено.
    let mut short = ev.clone();
    if let DomainEvent::SessionClosed { session, .. } = &mut short {
        session.id = SessionId::from(focus_api_types::Uuid::now_v7());
        session.ended_at = Some(session.started_at + Duration::seconds(10));
    }
    e.configure(json!({"worklog": {"work": true, "interrupted": false}}))
        .await;
    e.plugin.on_event(&short, &e.ctx()).await.unwrap();
    assert_eq!(
        e.requests("POST", "/rest/api/2/issue/ABC-1/worklog")
            .await
            .len(),
        1
    );
    e.configure(json!({"worklog": {"work": true, "interrupted": true}}))
        .await;
    Mock::given(method("POST"))
        .and(path("/rest/api/2/issue/ABC-1/worklog"))
        .and(body_json(json!({
            "started": "2026-10-24T09:00:00.000+0000",
            "timeSpentSeconds": 60,
            "comment": item.title,
        })))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({"id": 7})))
        .mount(&e.jira)
        .await;
    e.plugin.on_event(&short, &e.ctx()).await.unwrap();
    assert_eq!(
        e.requests("POST", "/rest/api/2/issue/ABC-1/worklog")
            .await
            .len(),
        2
    );
}

#[tokio::test]
async fn server_errors_are_retried() {
    let e = env().await;
    let r = e.imported().await;
    Mock::given(method("GET"))
        .and(path("/rest/api/2/issue/ABC-1"))
        .respond_with(ResponseTemplate::new(503))
        .mount(&e.jira)
        .await;
    e.app
        .service
        .create_task(NewTask::child(r.task.id, "Новый"))
        .await
        .unwrap();
    e.deliver().await;
    let st = e.app.service.plugin_status().await.unwrap();
    assert!(!st[0].healthy);
    assert_eq!(st[0].outbox_pending, 1);
    assert!(
        st[0].last_error.as_deref().unwrap().contains("503"),
        "{st:?}"
    );
    // Jira ожила — повтор проходит.
    e.jira.reset().await;
    e.mount_issue("ABC-1", SAMPLE).await;
    e.mount_put("ABC-1").await;
    e.clock.advance(Duration::seconds(10));
    e.deliver().await;
    let st = e.app.service.plugin_status().await.unwrap();
    assert!(st[0].healthy && st[0].outbox_pending == 0, "{st:?}");
}

#[tokio::test]
async fn mcp_tools() {
    let e = env().await;
    let tools = e.plugin.mcp_tools();
    let tool = |n: &str| tools.iter().find(|t| t.name == n).unwrap().clone();
    let out = tool("jira_status").call(Value::Null).await.unwrap();
    assert_eq!(out.data["configured"], false);
    assert!(out.summary.contains("не настроена"), "{}", out.summary);

    e.configure(json!({"checklist_field_id": FIELD})).await;
    e.mount_issue("ABC-1", SAMPLE).await;
    let out = tool("jira_status").call(Value::Null).await.unwrap();
    assert_eq!(out.data["configured"], true);
    assert_eq!(out.data["base_url"], e.jira.uri());
    assert!(!out.data.to_string().contains("pat-secret"));
    let out = tool("jira_import")
        .call(json!({"url": "ABC-1"}))
        .await
        .unwrap();
    assert_eq!(out.data["created"], true);
    assert_eq!(out.data["added_items"], 6);
    assert!(tool("jira_import").call(json!({})).await.is_err());
}
