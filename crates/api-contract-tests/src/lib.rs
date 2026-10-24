//! Сценарии поведения Focus Flow против любой реализации [`FocusApi`].
//!
//! Это обычные async-функции, а не `#[test]`: их вызывают и тесты клиента
//! (на `MockApi`), и тесты сервиса (на реализации поверх Axum-роутера).
//! Каждый сценарий возвращает `Err` с понятным текстом на русском.
//!
//! Требования к бэкенду: изначально пустой (свежая БД/мок), настройки по
//! умолчанию. Сценарии сами создают свои задачи и перед стартом завершают
//! активную работу/планирование. Отдых пропустить нельзя, поэтому сценарий,
//! оставляющий активный отдых, в [`run_all`] идёт последним.
//!
//! Если у бэкенда управляемые часы — передайте [`TestClock`] в
//! [`run_all_with_clock`]: тогда сценарии ещё и сдвигают время и проверяют
//! точные длительности (норма, отдых).

mod helpers;
mod scenarios;

use focus_api_types::FocusApi;

pub use helpers::TestClock;
pub use scenarios::*;

/// Результат одного сценария: имя и `Ok` или текст ошибки.
pub type ScenarioResult = (&'static str, Result<(), String>);

/// Прогоняет все сценарии по очереди на одном бэкенде (реальное время).
pub async fn run_all(api: &dyn FocusApi) -> Vec<ScenarioResult> {
    run(api, None).await
}

/// То же, что [`run_all`], но с управляемыми часами — проверки длительностей точные.
pub async fn run_all_with_clock(api: &dyn FocusApi, clock: &dyn TestClock) -> Vec<ScenarioResult> {
    run(api, Some(clock)).await
}

/// Проверяет, что все сценарии прошли; иначе — текст со списком провалов.
/// Удобно в `#[test]`: `assert_all_passed(&run_all(&api).await).unwrap()`.
pub fn assert_all_passed(results: &[ScenarioResult]) -> Result<(), String> {
    let failed: Vec<String> = results
        .iter()
        .filter_map(|(name, r)| r.as_ref().err().map(|e| format!("  {name}: {e}")))
        .collect();
    if failed.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "провалено {} из {} сценариев:\n{}",
            failed.len(),
            results.len(),
            failed.join("\n")
        ))
    }
}

async fn run(api: &dyn FocusApi, clock: Option<&dyn TestClock>) -> Vec<ScenarioResult> {
    vec![
        ("start_work_opens_day", start_work_opens_day(api).await),
        (
            "start_work_unknown_item_not_found",
            start_work_unknown_item_not_found(api).await,
        ),
        (
            "start_work_on_task_with_children_requires_item",
            start_work_on_task_with_children_requires_item(api).await,
        ),
        (
            "second_start_while_active_conflicts",
            second_start_while_active_conflicts(api).await,
        ),
        (
            "interrupt_ends_work_as_interrupted",
            interrupt_ends_work_as_interrupted(api).await,
        ),
        (
            "session_end_without_active_fails",
            session_end_without_active_fails(api).await,
        ),
        (
            "closed_day_blocks_sessions",
            closed_day_blocks_sessions(api).await,
        ),
        ("done_then_planning", done_then_planning(api).await),
        (
            "planning_counts_in_norm",
            planning_counts_in_norm(api, clock).await,
        ),
        // Последним: оставляет активный отдых, который нельзя пропустить.
        (
            "done_then_rest_rest_not_in_norm",
            done_then_rest_rest_not_in_norm(api, clock).await,
        ),
    ]
}
