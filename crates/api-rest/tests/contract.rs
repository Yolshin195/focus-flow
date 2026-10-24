//! Все сценарии `focus-api-contract-tests` против сервиса через REST-роутер.

mod common;

use common::{Env, EnvClock, RouterApi};
use focus_api_contract_tests::{assert_all_passed, run_all_with_clock};

#[tokio::test]
async fn contract_scenarios_pass_over_rest() {
    let env = Env::new().await;
    let api = RouterApi(&env);
    let results = run_all_with_clock(&api, &EnvClock(env.clock.clone())).await;
    assert_eq!(results.len(), 10);
    assert_all_passed(&results).unwrap_or_else(|e| panic!("{e}"));
}
