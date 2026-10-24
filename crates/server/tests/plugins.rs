//! Плагинная система на БД в памяти и управляемых часах: доставка outbox,
//! повторы с задержкой, порядок, выключенный плагин, импорт через
//! `TaskSource`, маршруты плагина и `/api/plugins/status`.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, LazyLock, Mutex};

use async_trait::async_trait;
use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use axum::routing::get;
use chrono::{Duration, TimeZone, Utc};
use focus_api_types::{ApiError, NewTask, ServerEvent, SettingsDto, TaskStatus};
use focus_core::{Clock as _, CoreError, ManualClock};
use focus_plugin_api::{
    DomainEvent, ImportedItem, ImportedTask, Migrator, Plugin, PluginCtx, TaskSource,
};
use focus_server::{App, Config, Registration, router};
use focus_storage::Storage;
use http_body_util::BodyExt as _;
use serde_json::{Value, json};
use sqlx::SqlSafeStr as _;
use sqlx::migrate::{Migration, MigrationType};
use tower::ServiceExt as _;

const TOKEN: &str = "t";

static FAKE_MIGRATOR: LazyLock<Migrator> = LazyLock::new(|| {
    Migrator::with_migrations(vec![Migration::new(
        1,
        "fake".into(),
        MigrationType::Simple,
        "CREATE TABLE IF NOT EXISTS fake_seen (event TEXT NOT NULL)".into_sql_str(),
        false,
    )])
});

/// Фейковый плагин: записывает события (и в свою таблицу), первые
/// `fail` доставок падают.
struct Fake {
    id: &'static str,
    fail: AtomicU32,
    seen: Mutex<Vec<String>>,
}

impl Fake {
    fn new(id: &'static str) -> Arc<Self> {
        Arc::new(Self {
            id,
            fail: AtomicU32::new(0),
            seen: Mutex::default(),
        })
    }
    fn seen(&self) -> Vec<String> {
        self.seen.lock().unwrap().clone()
    }
}

fn describe(ev: &DomainEvent) -> String {
    match ev {
        DomainEvent::ItemCreated { task, .. } => format!("created:{}", task.title),
        DomainEvent::ItemCompleted { task, .. } => format!("completed:{}", task.title),
        DomainEvent::SessionClosed { session, .. } => format!("closed:{:?}", session.kind),
        DomainEvent::TaskImported { task, .. } => format!("imported:{}", task.title),
        DomainEvent::Unknown => "unknown".into(),
    }
}

#[async_trait]
impl Plugin for Fake {
    fn id(&self) -> &'static str {
        self.id
    }

    fn migrations(&self) -> Option<&'static Migrator> {
        Some(&FAKE_MIGRATOR)
    }

    async fn on_event(&self, event: &DomainEvent, ctx: &PluginCtx) -> anyhow::Result<()> {
        if self
            .fail
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1))
            .is_ok()
        {
            anyhow::bail!("сеть недоступна");
        }
        let d = describe(event);
        sqlx::query("INSERT INTO fake_seen (event) VALUES (?)")
            .bind(&d)
            .execute(ctx.pool())
            .await?;
        self.seen.lock().unwrap().push(d);
        Ok(())
    }

    fn routes(&self, ctx: PluginCtx) -> Option<axum::Router> {
        Some(axum::Router::new().route(
            "/ping",
            get(move || {
                let ctx = ctx.clone();
                async move {
                    let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM fake_seen")
                        .fetch_one(ctx.pool())
                        .await
                        .unwrap_or(-1);
                    axum::Json(json!({"plugin": ctx.id(), "seen": n}))
                }
            }),
        ))
    }
}

/// Фейковый источник задач: ссылки с префиксом `prefix`.
struct Source {
    id: &'static str,
    prefix: &'static str,
}

#[async_trait]
impl Plugin for Source {
    fn id(&self) -> &'static str {
        self.id
    }
    async fn on_event(&self, _: &DomainEvent, _: &PluginCtx) -> anyhow::Result<()> {
        Ok(())
    }
}

#[async_trait]
impl TaskSource for Source {
    fn matches(&self, url: &str) -> bool {
        url.starts_with(self.prefix)
    }
    async fn import(&self, url: &str, ctx: &PluginCtx) -> anyhow::Result<ImportedTask> {
        if url.ends_with("/bad") {
            return Err(CoreError::Validation("нет такой задачи".into()).into());
        }
        if url.ends_with("/down") {
            anyhow::bail!("сервер не отвечает");
        }
        Ok(ImportedTask {
            source: "подменит реестр".into(),
            url: url.to_owned(),
            title: format!("{} из {}", url.trim_start_matches(self.prefix), ctx.id()),
            notes: None,
            items: vec![
                ImportedItem {
                    title: "первый".into(),
                    status: TaskStatus::Done,
                },
                ImportedItem {
                    title: "второй".into(),
                    status: TaskStatus::Planned,
                },
            ],
        })
    }
}

struct Env {
    app: App,
    clock: Arc<ManualClock>,
    _dir: tempfile::TempDir,
}

async fn env(plugins: Vec<Registration>) -> Env {
    let clock = Arc::new(ManualClock::new(
        Utc.with_ymd_and_hms(2026, 10, 24, 9, 0, 0).unwrap(),
    ));
    let dir = tempfile::tempdir().unwrap();
    let storage = Storage::open_in_memory().await.unwrap();
    let app = App::from_parts(
        Config::with_data_dir(dir.path()),
        storage,
        clock.clone(),
        plugins,
        TOKEN,
    )
    .await
    .unwrap();
    Env {
        app,
        clock,
        _dir: dir,
    }
}

impl Env {
    async fn enable(&self, ids: &[&str], on: bool) {
        let mut s: SettingsDto = self.app.service.settings().await.unwrap();
        for id in ids {
            s.plugins.insert((*id).into(), json!({ "enabled": on }));
        }
        self.app.service.update_settings(s).await.unwrap();
    }

    async fn add(&self, title: &str) {
        self.app
            .service
            .create_task(NewTask::new(title))
            .await
            .unwrap();
    }

    async fn deliver(&self) -> usize {
        self.app.plugins.deliver_due().await.unwrap()
    }

    async fn http(
        &self,
        method: &str,
        uri: &str,
        body: Option<Value>,
        token: bool,
    ) -> (StatusCode, Value) {
        let mut b = Request::builder()
            .method(method)
            .uri(uri)
            .header(header::HOST, "127.0.0.1:7878");
        if token {
            b = b.header(header::AUTHORIZATION, format!("Bearer {TOKEN}"));
        }
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
        let v = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        (status, v)
    }
}

#[tokio::test]
async fn enabled_plugin_receives_events_in_order() {
    let fake = Fake::new("fake");
    let e = env(vec![Registration::Plugin(fake.clone())]).await;
    e.enable(&["fake"], true).await;
    e.add("a").await;
    e.add("b").await;
    assert_eq!(e.deliver().await, 2);
    assert_eq!(fake.seen(), ["created:a", "created:b"]);
    // Повторно не доставляется.
    assert_eq!(e.deliver().await, 0);
    assert_eq!(fake.seen().len(), 2);
}

#[tokio::test]
async fn disabled_plugin_receives_nothing() {
    let fake = Fake::new("fake");
    let e = env(vec![Registration::Plugin(fake.clone())]).await;
    // По умолчанию выключен.
    e.add("до включения").await;
    assert_eq!(e.deliver().await, 0);
    e.enable(&["fake"], true).await;
    e.add("при включённом").await;
    e.enable(&["fake"], false).await;
    e.add("после выключения").await;
    // Выключен: даже накопленное не доставляется.
    assert_eq!(e.deliver().await, 0);
    assert!(fake.seen().is_empty());
    // Включили снова — доставлено только то, что публиковалось при включённом.
    e.enable(&["fake"], true).await;
    e.deliver().await;
    assert_eq!(fake.seen(), ["created:при включённом"]);
}

#[tokio::test]
async fn failure_retries_with_backoff_and_keeps_order() {
    let fake = Fake::new("fake");
    let other = Fake::new("other");
    fake.fail.store(2, Ordering::SeqCst);
    let e = env(vec![
        Registration::Plugin(fake.clone()),
        Registration::Plugin(other.clone()),
    ])
    .await;
    e.enable(&["fake", "other"], true).await;
    let mut rx = e.app.events.subscribe();
    e.add("a").await;
    e.add("b").await;

    // Первая попытка «a» падает; «b» ждёт за ней; другой плагин не блокируется.
    e.deliver().await;
    assert!(fake.seen().is_empty());
    assert_eq!(other.seen(), ["created:a", "created:b"]);
    match rx.recv().await.unwrap() {
        ServerEvent::PluginError { plugin, message } => {
            assert_eq!(plugin, "fake");
            assert!(message.contains("сеть недоступна"), "{message}");
        }
        ev => panic!("ожидался PluginError, получено {ev:?}"),
    }
    let st = e.app.service.plugin_status().await.unwrap();
    let f = st.iter().find(|s| s.id == "fake").unwrap();
    assert!(f.enabled && !f.healthy);
    assert_eq!(f.outbox_pending, 2);
    assert_eq!(f.last_error.as_deref(), Some("сеть недоступна"));
    assert_eq!(f.last_error_at, Some(e.clock.now()));

    // До истечения 10 с — ничего; через 10 с — вторая неудача (задержка 20 с).
    e.clock.advance(Duration::seconds(9));
    assert_eq!(e.deliver().await, 0);
    e.clock.advance(Duration::seconds(1));
    assert_eq!(e.deliver().await, 1);
    assert!(fake.seen().is_empty());
    e.clock.advance(Duration::seconds(19));
    assert_eq!(e.deliver().await, 0);
    e.clock.advance(Duration::seconds(1));
    assert_eq!(e.deliver().await, 2);
    assert_eq!(fake.seen(), ["created:a", "created:b"]);

    let st = e.app.service.plugin_status().await.unwrap();
    let f = st.iter().find(|s| s.id == "fake").unwrap();
    assert!(f.healthy);
    assert_eq!(f.outbox_pending, 0);
}

#[tokio::test]
async fn import_routes_to_matching_enabled_source() {
    let e = env(vec![
        Registration::Source(Arc::new(Source {
            id: "src_a",
            prefix: "a://",
        })),
        Registration::Source(Arc::new(Source {
            id: "src_b",
            prefix: "b://",
        })),
        Registration::Source(Arc::new(Source {
            id: "src_any",
            prefix: "",
        })),
    ])
    .await;
    // Нет включённых источников.
    let err = e.app.service.import("b://X-1").await.unwrap_err();
    assert!(matches!(err, CoreError::PluginUnavailable(_)), "{err:?}");

    e.enable(&["src_a", "src_b"], true).await;
    let r = e.app.service.import("b://X-1").await.unwrap();
    assert_eq!(r.source, "src_b");
    assert!(r.created);
    assert_eq!(r.task.title, "X-1 из src_b");
    assert_eq!(r.task.children.len(), 2);
    assert_eq!(r.task.children[0].status, TaskStatus::Done);
    // Повторно — не дублирует.
    let r = e.app.service.import("b://X-1").await.unwrap();
    assert!(!r.created);
    assert_eq!(r.added_items, 0);

    // Ни один включённый не подходит (src_any выключен).
    let err = e.app.service.import("c://Y").await.unwrap_err();
    assert!(matches!(err, CoreError::PluginUnavailable(_)), "{err:?}");
    // Включённый «ловит всё» — после более специфичных по порядку регистрации.
    e.enable(&["src_any"], true).await;
    assert_eq!(
        e.app.service.import("c://Y").await.unwrap().source,
        "src_any"
    );
    assert_eq!(e.app.service.import("a://Z").await.unwrap().source, "src_a");

    // Ошибки источника: ядро — как есть, прочее — plugin_unavailable.
    let err = e.app.service.import("a://bad").await.unwrap_err();
    assert_eq!(err, CoreError::Validation("нет такой задачи".into()));
    let err = e.app.service.import("a://down").await.unwrap_err();
    match err {
        CoreError::PluginUnavailable(m) => assert!(m.contains("src_a: сервер не отвечает"), "{m}"),
        other => panic!("{other:?}"),
    }

    // Через REST.
    let (code, body) = e
        .http("POST", "/api/import", Some(json!({"url": "a://R"})), true)
        .await;
    assert_eq!(code, StatusCode::OK, "{body}");
    assert_eq!(body["source"], "src_a");
    let (code, body) = e
        .http(
            "POST",
            "/api/import",
            Some(json!({"url": "a://down"})),
            true,
        )
        .await;
    assert_eq!(code, StatusCode::SERVICE_UNAVAILABLE);
    let err: ApiError = serde_json::from_value(body).unwrap();
    assert!(matches!(err, ApiError::PluginUnavailable(_)));
}

#[tokio::test]
async fn plugin_routes_mounted_behind_guard() {
    let fake = Fake::new("fake");
    let e = env(vec![Registration::Plugin(fake.clone())]).await;

    // Выключен — 503.
    let (code, body) = e.http("GET", "/api/plugins/fake/ping", None, true).await;
    assert_eq!(code, StatusCode::SERVICE_UNAVAILABLE, "{body}");
    assert_eq!(body["code"], "plugin_unavailable");

    e.enable(&["fake"], true).await;
    e.add("x").await;
    e.deliver().await;
    // Без токена — 401, с токеном — ответ плагина (таблица плагина создана миграцией).
    let (code, _) = e.http("GET", "/api/plugins/fake/ping", None, false).await;
    assert_eq!(code, StatusCode::UNAUTHORIZED);
    let (code, body) = e.http("GET", "/api/plugins/fake/ping", None, true).await;
    assert_eq!(code, StatusCode::OK);
    assert_eq!(body, json!({"plugin": "fake", "seen": 1}));

    // Статус плагинов через REST.
    let (code, body) = e.http("GET", "/api/plugins/status", None, true).await;
    assert_eq!(code, StatusCode::OK);
    assert_eq!(body[0]["id"], "fake");
    assert_eq!(body[0]["enabled"], true);
    assert_eq!(body[0]["healthy"], true);
    assert_eq!(body[0]["outbox_pending"], 0);
    // Остальные REST-маршруты не задеты.
    let (code, _) = e.http("GET", "/api/status", None, true).await;
    assert_eq!(code, StatusCode::OK);
}

#[tokio::test]
async fn settings_and_secrets_from_ctx() {
    struct Probe;
    #[async_trait]
    impl Plugin for Probe {
        fn id(&self) -> &'static str {
            "probe"
        }
        async fn on_event(&self, _: &DomainEvent, ctx: &PluginCtx) -> anyhow::Result<()> {
            let s = ctx.settings();
            anyhow::ensure!(s.enabled().await?);
            let mut v = s.get().await?;
            v["seen"] = json!(true);
            s.set(&v).await?;
            ctx.secrets().set("token", "секрет")?;
            // Через сервис — создать пункт.
            ctx.service()?
                .create_task(NewTask::new("от плагина"))
                .await?;
            Ok(())
        }
    }
    let e = env(vec![Registration::Plugin(Arc::new(Probe))]).await;
    e.enable(&["probe"], true).await;
    e.add("x").await;
    assert_eq!(e.deliver().await, 1);
    let s = e.app.service.settings().await.unwrap();
    assert_eq!(s.plugins["probe"], json!({"enabled": true, "seen": true}));
    let path = e.app.config.secrets_dir().join("probe").join("token");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "секрет");
    let tree = e.app.service.list_tasks().await.unwrap();
    assert!(tree.iter().any(|t| t.title == "от плагина"));
    // Плагин остался включён после собственной записи настроек.
    assert!(e.app.plugins.is_enabled("probe"));
}

#[tokio::test]
async fn bad_registrations_are_rejected() {
    let storage = Storage::open_in_memory().await.unwrap();
    let clock = Arc::new(ManualClock::new(Utc::now()));
    let r = App::from_parts(
        Config::with_data_dir("/nonexistent"),
        storage,
        clock,
        vec![
            Registration::Plugin(Fake::new("dup")),
            Registration::Plugin(Fake::new("dup")),
        ],
        TOKEN,
    )
    .await;
    assert!(r.is_err());
}
