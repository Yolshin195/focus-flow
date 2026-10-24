//! Сценарии контракта на настоящем сервисе по HTTP через `HttpApi`.
//!
//! По умолчанию пропускается (`#[ignore]`). Запуск — на свежем data dir:
//!
//! ```sh
//! cargo run -p focus-flow -- serve --data-dir /tmp/ff-ui-it --addr 127.0.0.1:7979 &
//! export FOCUS_TEST_ADDR=127.0.0.1:7979
//! export FOCUS_TEST_TOKEN=$(cargo run -q -p focus-flow -- token --data-dir /tmp/ff-ui-it)
//! cargo test -p ui --test http_contract -- --ignored --nocapture
//! ```

use std::time::Duration;

use focus_api_contract_tests::{assert_all_passed, run_all};
use focus_api_types::{ApiError, FocusApi, ServerEvent};
use futures_util::StreamExt;
use ui::HttpApi;

fn api() -> HttpApi {
    let addr = std::env::var("FOCUS_TEST_ADDR").expect("FOCUS_TEST_ADDR не задан");
    let token = std::env::var("FOCUS_TEST_TOKEN").expect("FOCUS_TEST_TOKEN не задан");
    HttpApi::new(addr, token)
}

#[tokio::test(flavor = "current_thread")]
#[ignore = "нужен запущенный сервис: FOCUS_TEST_ADDR и FOCUS_TEST_TOKEN"]
async fn contract_scenarios_pass_over_http() {
    let api = api();

    // SSE: первым событием приходит текущий статус.
    let events = api.events().await.expect("SSE не открылся");
    let mut events = std::pin::pin!(events);
    let first = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            match events.next().await {
                Some(Ok(evs)) if !evs.is_empty() => return evs,
                Some(Ok(_)) => continue,
                other => panic!("SSE закрылся: {other:?}"),
            }
        }
    })
    .await
    .expect("нет первого события SSE");
    assert!(
        matches!(first[0], ServerEvent::StatusChanged(_)),
        "{first:?}"
    );

    let results = run_all(&api).await;
    for (name, r) in &results {
        println!("{name}: {}", if r.is_ok() { "ok" } else { "FAIL" });
    }
    assert_all_passed(&results).unwrap_or_else(|e| panic!("{e}"));

    // События сценариев тоже дошли по SSE.
    let got = tokio::time::timeout(Duration::from_secs(5), async {
        let mut kinds = Vec::new();
        while let Some(Ok(evs)) = events.next().await {
            kinds.extend(evs.iter().map(ServerEvent::name));
            if kinds.contains(&"session_ended") && kinds.contains(&"task_changed") {
                break;
            }
        }
        kinds
    })
    .await
    .expect("события сценариев не пришли по SSE");
    assert!(got.contains(&"status_changed"), "{got:?}");

    // Неверный токен → Unauthorized из JSON-тела.
    let bad = HttpApi::new(api.base_url(), "nope");
    assert_eq!(bad.get_status().await, Err(ApiError::Unauthorized));
}

#[tokio::test(flavor = "current_thread")]
async fn unreachable_service_is_transport_error() {
    // Порт 9 (discard) на loopback никто не слушает.
    let api = HttpApi::new("127.0.0.1:9", "t");
    assert!(matches!(
        api.get_status().await,
        Err(ApiError::Transport(_))
    ));
    assert!(matches!(
        api.events().await.err(),
        Some(ApiError::Transport(_))
    ));
}
