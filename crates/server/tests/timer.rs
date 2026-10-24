//! Фоновый таймер на БД в памяти и управляемых часах: закрывает работу по
//! лимиту, стартует отдых, рассылает события и показывает уведомления.

use std::sync::{Arc, Mutex};
use std::time::Duration as StdDuration;

use chrono::{Duration, TimeZone, Utc};
use focus_api_types::{ActiveSession, EndReason, NewTask, ServerEvent, SessionKind};
use focus_core::{ManualClock, NoTaskSources};
use focus_server::{App, Config, Notifier, run_timer};
use focus_storage::Storage;
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;

#[derive(Default)]
struct Recorder(Mutex<Vec<String>>);

impl Notifier for Recorder {
    fn notify(&self, _title: &str, body: &str) {
        self.0.lock().unwrap().push(body.to_owned());
    }
}

async fn next_event(rx: &mut broadcast::Receiver<ServerEvent>) -> ServerEvent {
    tokio::time::timeout(StdDuration::from_secs(5), rx.recv())
        .await
        .expect("событие не пришло за 5 с")
        .expect("шина закрыта")
}

async fn wait_for(rec: &Recorder, n: usize) -> Vec<String> {
    for _ in 0..100 {
        let got = rec.0.lock().unwrap().clone();
        if got.len() >= n {
            return got;
        }
        tokio::time::sleep(StdDuration::from_millis(20)).await;
    }
    panic!("уведомлений меньше {n}: {:?}", rec.0.lock().unwrap());
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
        Arc::new(NoTaskSources),
        "t",
    );
    let item = app
        .service
        .create_task(NewTask::new("пункт"))
        .await
        .unwrap();
    app.service.start_work(item.id).await.unwrap();

    let rec = Arc::new(Recorder::default());
    let mut rx = app.events.subscribe();
    let cancel = CancellationToken::new();
    let timer = tokio::spawn(run_timer(
        app.clone(),
        rec.clone(),
        StdDuration::from_millis(20),
        cancel.clone(),
    ));

    // Лимит работы истёк → SessionEnded(work, limit), затем StatusChanged с отдыхом.
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
            assert!(
                matches!(s.active, Some(ActiveSession::Rest { .. })),
                "{s:?}"
            );
        }
        other => panic!("ожидался StatusChanged, получено {other:?}"),
    }
    assert_eq!(
        wait_for(&rec, 1).await,
        ["Время работы вышло — отдохни 10 минут"]
    );

    // Отдых кончился → SessionEnded(rest), StatusChanged без сессии.
    clock.advance(Duration::minutes(10));
    match next_event(&mut rx).await {
        ServerEvent::SessionEnded { session } => assert_eq!(session.kind, SessionKind::Rest),
        other => panic!("ожидался SessionEnded, получено {other:?}"),
    }
    match next_event(&mut rx).await {
        ServerEvent::StatusChanged(s) => assert_eq!(s.active, None),
        other => panic!("ожидался StatusChanged, получено {other:?}"),
    }
    assert_eq!(
        wait_for(&rec, 2).await[1],
        "Отдых окончен — выбери следующую задачу"
    );

    cancel.cancel();
    tokio::time::timeout(StdDuration::from_secs(5), timer)
        .await
        .expect("таймер не остановился")
        .unwrap();
}
