//! Доступ к данным: только через трейт [`FocusApi`] из `focus-api-types`.

mod http;
mod mock;

use std::rc::Rc;

pub use focus_api_types::FocusApi;
pub use http::HttpApi;
pub use mock::{jira_key, MockApi, MockScenario};

/// Реализация `FocusApi` в контексте Dioxus (`MockApi` или `HttpApi`).
///
/// У `HttpApi` есть ещё поток событий SSE — тогда клиент живёт на событиях
/// и редком страховочном опросе; у мока событий нет — опрос раз в секунду.
#[derive(Clone)]
pub struct ApiHandle(pub Rc<dyn FocusApi>, Option<HttpApi>);

impl ApiHandle {
    /// Обёртка над реализацией без событий (мок, тесты).
    pub fn new(api: impl FocusApi + 'static) -> Self {
        Self(Rc::new(api), None)
    }

    /// Настоящий сервис: команды по REST, живые обновления по SSE.
    pub fn http(api: HttpApi) -> Self {
        Self(Rc::new(api.clone()), Some(api))
    }

    /// Клиент SSE, если реализация — настоящий сервис.
    pub fn events(&self) -> Option<&HttpApi> {
        self.1.as_ref()
    }

    /// `true` — данные идут от сервиса (он же сам показывает уведомления).
    pub fn is_remote(&self) -> bool {
        self.1.is_some()
    }
}
