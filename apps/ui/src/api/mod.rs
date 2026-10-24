//! Доступ к данным: только через трейт [`FocusApi`] из `focus-api-types`.

mod mock;

use std::rc::Rc;

pub use focus_api_types::FocusApi;
pub use mock::{jira_key, MockApi, MockScenario};

/// Реализация `FocusApi` в контексте Dioxus (`MockApi` или, позже, `HttpApi`).
#[derive(Clone)]
pub struct ApiHandle(pub Rc<dyn FocusApi>);

impl ApiHandle {
    /// Обёртка над реализацией.
    pub fn new(api: impl FocusApi + 'static) -> Self {
        Self(Rc::new(api))
    }
}
