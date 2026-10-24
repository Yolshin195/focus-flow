//! MCP поверх роутера без сети: сырой JSON-RPC (Streamable HTTP, без
//! сессий) через тот же middleware доступа, что у REST.

use std::sync::Arc;
use std::time::Duration as StdDuration;

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use chrono::{Duration, TimeZone, Utc};
use focus_api_mcp::{McpState, router};
use focus_api_rest::Access;
use focus_api_types::ServerEvent;
use focus_core::{FocusService, ManualClock, NoTaskSources};
use focus_plugin_api::{McpTool, PluginTools, ToolOutput};
use focus_storage::Storage;
use http_body_util::BodyExt as _;
use serde_json::{Value, json};
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;
use tower::ServiceExt as _;

const TOKEN: &str = "mcp-token";

struct FakeTools;

impl PluginTools for FakeTools {
    fn tools(&self) -> Vec<McpTool> {
        vec![
            McpTool::new(
                "fake_echo",
                "эхо",
                json!({"type": "object", "properties": {"x": {"type": "integer"}}}),
                |a| async move { ToolOutput::new("эхо", &a) },
            ),
            // Совпадает с инструментом ядра — должен быть пропущен.
            McpTool::new(
                "get_status",
                "подмена",
                json!({"type": "object"}),
                |_| async { anyhow::bail!("не должен вызываться") },
            ),
        ]
    }
}

struct Env {
    clock: Arc<ManualClock>,
    events: broadcast::Sender<ServerEvent>,
    router: Router,
    service: Arc<FocusService>,
    next_id: std::sync::atomic::AtomicU64,
}

async fn env() -> Env {
    let clock = Arc::new(ManualClock::new(
        Utc.with_ymd_and_hms(2026, 10, 24, 9, 0, 0).unwrap(),
    ));
    let storage = Storage::open_in_memory().await.unwrap();
    let service = Arc::new(FocusService::new(
        storage.ports(clock.clone(), Arc::new(NoTaskSources)),
    ));
    let (events, _) = broadcast::channel(64);
    let state = McpState::new(
        service.clone(),
        events.clone(),
        Arc::new(FakeTools),
        CancellationToken::new(),
    );
    let router = router(state).layer(axum::middleware::from_fn_with_state(
        Arc::new(Access::new(TOKEN)),
        focus_api_rest::guard::guard,
    ));
    Env {
        clock,
        events,
        router,
        service,
        next_id: 1.into(),
    }
}

impl Env {
    async fn post(&self, body: Value, token: Option<&str>) -> (StatusCode, Value) {
        let mut b = Request::builder()
            .method("POST")
            .uri("/mcp")
            .header(header::HOST, "127.0.0.1:7878")
            .header(header::CONTENT_TYPE, "application/json")
            .header(header::ACCEPT, "application/json, text/event-stream");
        if let Some(t) = token {
            b = b.header(header::AUTHORIZATION, format!("Bearer {t}"));
        }
        let req = b.body(Body::from(body.to_string())).unwrap();
        let resp = self.router.clone().oneshot(req).await.unwrap();
        let status = resp.status();
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let v = serde_json::from_slice(&bytes)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&bytes).into_owned()));
        (status, v)
    }

    async fn rpc(&self, method: &str, params: Value) -> Value {
        let id = self
            .next_id
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let (status, v) = self
            .post(
                json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}),
                Some(TOKEN),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{method}: {v}");
        assert_eq!(v["id"], id, "{v}");
        v
    }

    /// `tools/call` → (сводка, JSON, isError).
    async fn call(&self, name: &str, args: Value) -> (String, Value, bool) {
        let v = self
            .rpc("tools/call", json!({"name": name, "arguments": args}))
            .await;
        let r = &v["result"];
        assert!(r.is_object(), "{name}: {v}");
        let content = r["content"].as_array().unwrap();
        let summary = content[0]["text"].as_str().unwrap().to_owned();
        let data = serde_json::from_str(content[1]["text"].as_str().unwrap()).unwrap();
        (summary, data, r["isError"] == true)
    }
}

#[tokio::test]
async fn initialize_and_list_tools() {
    let e = env().await;
    let v = e
        .rpc(
            "initialize",
            json!({
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": {"name": "test", "version": "1"}
            }),
        )
        .await;
    let r = &v["result"];
    assert_eq!(r["serverInfo"]["name"], "focus-flow");
    assert!(r["capabilities"]["tools"].is_object());
    assert!(r["instructions"].as_str().unwrap().contains("Паузы нет"));

    let v = e.rpc("tools/list", json!({})).await;
    let names: Vec<&str> = v["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        [
            "get_status",
            "open_day",
            "close_day",
            "list_tasks",
            "add_task",
            "update_task",
            "start_work",
            "finish_item",
            "interrupt",
            "start_planning",
            "stop",
            "start_rest",
            "get_day_report",
            "fake_echo",
        ]
    );
    for t in v["result"]["tools"].as_array().unwrap() {
        assert_eq!(t["inputSchema"]["type"], "object", "{t}");
        assert!(!t["description"].as_str().unwrap().is_empty());
    }
    let (s, d, err) = e.call("fake_echo", json!({"x": 7})).await;
    assert_eq!((s.as_str(), d["x"].as_i64(), err), ("эхо", Some(7), false));
}

#[tokio::test]
async fn guard_applies_to_mcp() {
    let e = env().await;
    let body = json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"});
    let (status, _) = e.post(body.clone(), None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, _) = e.post(body, Some("wrong")).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn work_flow_and_status_events() {
    let e = env().await;
    let mut rx = e.events.subscribe();

    let (s, st, _) = e.call("get_status", json!({})).await;
    assert!(s.starts_with("День не открыт"), "{s}");
    assert_eq!(st["active"], Value::Null);
    assert_eq!(st["ends_at"], Value::Null);
    assert_eq!(st["events"], json!([]));

    let (s, task, err) = e.call("add_task", json!({"title": "Релиз"})).await;
    assert!(!err && s.contains("«Релиз»"), "{s}");
    let tid = task["id"].as_str().unwrap().to_owned();
    let (_, item, _) = e
        .call(
            "add_task",
            json!({"title": "Написать changelog", "parent_id": tid, "notes": "кратко"}),
        )
        .await;
    let iid = item["id"].as_str().unwrap().to_owned();

    // Задачу с пунктами в работу не взять — ошибка инструмента с подсказкой.
    let (s, d, err) = e.call("start_work", json!({"item_id": tid})).await;
    assert!(err);
    assert_eq!(d["code"], "item_required");
    assert!(s.contains("list_tasks"), "{s}");

    let (s, st, err) = e.call("start_work", json!({"item_id": iid})).await;
    assert!(!err, "{s}");
    assert!(
        s.contains("Работа начата") && s.contains("осталось 50 мин"),
        "{s}"
    );
    assert_eq!(st["active"]["kind"], "work");
    assert_eq!(st["remaining_sec"], 3000);
    assert_eq!(st["ends_at"], "2026-10-24T09:50:00Z");
    // Команда публикует в шину, как REST.
    let mut names = Vec::new();
    while let Ok(ev) = rx.try_recv() {
        names.push(ev.name());
    }
    assert!(names.contains(&"status_changed"), "{names:?}");

    e.clock.advance(Duration::minutes(20));
    let (_, st, _) = e.call("get_status", json!({})).await;
    assert_eq!(st["remaining_sec"], 30 * 60);
    assert_eq!(st["active"]["item"], "Написать changelog");

    let (s, r, err) = e.call("finish_item", json!({"next": "rest"})).await;
    assert!(!err, "{s}");
    assert!(
        s.starts_with("Работа над «Написать changelog» закрыта (пункт готов, 20 мин)"),
        "{s}"
    );
    assert_eq!(r["ended"]["end_reason"], "task_done");
    assert_eq!(r["status"]["active"]["kind"], "rest");

    let (_, tree, _) = e.call("list_tasks", json!({})).await;
    // Пункт готов → скрыт; задача открыта, видно, что пункты у неё были.
    assert_eq!(tree.as_array().unwrap().len(), 1);
    assert_eq!(tree[0]["done_items"], 1);
    assert_eq!(tree[0].get("items"), None);
    let (_, tree, _) = e.call("list_tasks", json!({"include_done": true})).await;
    assert_eq!(tree[0]["items"][0]["status"], "done");

    // Отдых закончился по лимиту (как таймер) → get_status показывает событие.
    e.clock.advance(Duration::minutes(10));
    for ev in e.service.tick(e.clock_now()).await.unwrap() {
        let _ = e.events.send(ev);
    }
    // Ждём, пока лента прочитает шину.
    let mut found = Value::Null;
    for _ in 0..50 {
        let (s, st, _) = e.call("get_status", json!({})).await;
        if st["events"].as_array().is_some_and(|a| !a.is_empty()) {
            assert!(s.contains("С прошлого раза"), "{s}");
            found = st;
            break;
        }
        tokio::time::sleep(StdDuration::from_millis(20)).await;
    }
    let evs = found["events"].as_array().expect("событий нет");
    // finish_item сам опубликовал session_ended работы; затем отдых закрыт по лимиту.
    let kinds: Vec<(&str, &str)> = evs
        .iter()
        .map(|x| {
            (
                x["kind"].as_str().unwrap(),
                x["end_reason"].as_str().unwrap(),
            )
        })
        .collect();
    assert!(kinds.contains(&("rest", "limit")), "{kinds:?}");
    // Повторный вызов — новых событий нет.
    let (_, st, _) = e.call("get_status", json!({})).await;
    assert_eq!(st["events"], json!([]));

    let (s, rep, _) = e.call("get_day_report", json!({})).await;
    assert!(s.starts_with("2026-10-24: норма 0:20 из 8:00"), "{s}");
    assert_eq!(rep["totals"]["work_sec"], 1200);
    assert_eq!(rep["sessions"].as_array().unwrap().len(), 2);
    let (s, _, _) = e
        .call("get_day_report", json!({"date": "2026-10-23"}))
        .await;
    assert!(s.contains("не открывался"), "{s}");
}

#[tokio::test]
async fn other_commands_and_errors() {
    let e = env().await;
    let (s, _, err) = e.call("stop", json!({})).await;
    assert!(err && s.contains("нет активной сессии"), "{s}");

    let (_, st, _) = e.call("start_planning", json!({})).await;
    assert_eq!(st["active"]["kind"], "planning");
    assert_eq!(st["ends_at"], Value::Null);
    e.clock.advance(Duration::minutes(5));
    let (s, r, _) = e.call("stop", json!({})).await;
    assert!(
        s.starts_with("Планирование закрыта (остановлена, 5 мин)"),
        "{s}"
    );
    assert_eq!(r["ended"]["end_reason"], "stopped");

    let (_, item, _) = e.call("add_task", json!({"title": "x"})).await;
    let id = item["id"].as_str().unwrap();
    let (_, t, err) = e
        .call(
            "update_task",
            json!({"id": id, "title": "y", "notes": "n", "status": "cancelled"}),
        )
        .await;
    assert!(!err);
    assert_eq!(
        (t["title"].as_str(), t["status"].as_str()),
        (Some("y"), Some("cancelled"))
    );
    let (_, t, _) = e
        .call("update_task", json!({"id": id, "notes": null}))
        .await;
    assert_eq!(t.get("notes"), None);

    let (_, st, _) = e.call("start_rest", json!({})).await;
    assert_eq!(st["active"]["kind"], "rest");
    let (s, d, err) = e.call("close_day", json!({})).await;
    assert!(err, "{s}");
    assert_eq!(d["code"], "session_already_active");

    // Плохие аргументы → validation.
    let (_, d, err) = e.call("start_work", json!({"item": "?"})).await;
    assert!(err);
    assert_eq!(d["code"], "validation");

    // Неизвестный инструмент → ошибка протокола.
    let v = e
        .rpc("tools/call", json!({"name": "nope", "arguments": {}}))
        .await;
    assert!(v["error"].is_object(), "{v}");

    let (_, _, err) = e.call("interrupt", json!({})).await;
    assert!(err, "прервать можно только работу");
    let (_, _, err) = e.call("open_day", json!({})).await;
    assert!(!err);
}

impl Env {
    fn clock_now(&self) -> chrono::DateTime<Utc> {
        use focus_core::Clock as _;
        self.clock.now()
    }
}
