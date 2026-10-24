//! Фоновый таймер на БД в памяти и управляемых часах: закрывает работу по
//! лимиту (отдых сам не стартует — `rest_due`), закрывает отдых и рассылает
//! события.

use std::sync::Arc;
use std::time::Duration as StdDuration;

use chrono::{Duration, TimeZone, Utc};
use focus_api_types::{ActiveSession, EndReason, NewTask, ServerEvent, SessionKind};
use focus_core::ManualClock;
use focus_server::{App, Config, run_timer};
use focus_storage::Storage;
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;

async fn next_event(rx: &mut broadcast::Receiver<ServerEvent>) -> ServerEvent {
    tokio::time::timeout(StdDuration::from_secs(5), rx.recv())
        .await
        .expect("событие не пришло за 5 с")
        .expect("шина закрыта")
}

#[tokio::test]
async fn timer_closes_work_then_rest() {
    let clock = Arc::new(ManualClock::new(
        Utc.with_ymd_and_hms(2026, 10, 24, 9, 0, 0).unwrap(),
    ));
    let storage = Storage::open_in_memory().await.unwrap();
    let app = App::from_parts(
        Config::with_data_dir("/nonexistent"),
        storage,
        clock.clone(),
        Vec::new(),
        "t",
    )
    .await
    .unwrap();
    let item = app
        .service
        .create_task(NewTask::new("пункт"))
        .await
        .unwrap();
    app.service.start_work(item.id).await.unwrap();

    let mut rx = app.events.subscribe();
    let cancel = CancellationToken::new();
    let timer = tokio::spawn(run_timer(
        app.clone(),
        StdDuration::from_millis(20),
        cancel.clone(),
    ));

    // Лимит работы истёк → SessionEnded(work, limit), затем StatusChanged:
    // сессии нет, отдых ждёт пользователя.
    clock.advance(Duration::minutes(50));
    match next_event(&mut rx).await {
        ServerEvent::SessionEnded { session } => {
            assert_eq!(session.kind, SessionKind::Work);
            assert_eq!(session.end_reason, Some(EndReason::Limit));
            assert_eq!(session.duration_sec, 50 * 60);
        }
        other => panic!("ожидался SessionEnded, получено {other:?}"),
    }
    match next_event(&mut rx).await {
        ServerEvent::StatusChanged(s) => {
            assert_eq!(s.active, None);
            assert!(s.rest_due, "{s:?}");
        }
        other => panic!("ожидался StatusChanged, получено {other:?}"),
    }

    // Пользователь начал отдых; отдых кончился → SessionEnded(rest), StatusChanged без сессии.
    let st = app.service.start_rest().await.unwrap();
    assert!(matches!(st.active, Some(ActiveSession::Rest { .. })));
    clock.advance(Duration::minutes(10));
    match next_event(&mut rx).await {
        ServerEvent::SessionEnded { session } => assert_eq!(session.kind, SessionKind::Rest),
        other => panic!("ожидался SessionEnded, получено {other:?}"),
    }
    match next_event(&mut rx).await {
        ServerEvent::StatusChanged(s) => assert_eq!(s.active, None),
        other => panic!("ожидался StatusChanged, получено {other:?}"),
    }

    cancel.cancel();
    tokio::time::timeout(StdDuration::from_secs(5), timer)
        .await
        .expect("таймер не остановился")
        .unwrap();
}
