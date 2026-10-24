//! Доступ к данным: только через трейт [`FocusApi`] из `focus-api-types`.

mod http;
mod jira;
mod mock;

use std::rc::Rc;

pub use focus_api_types::FocusApi;
pub use http::HttpApi;
pub use jira::{
    JiraMode, JiraPatch, JiraSettingsApi, JiraState, JiraTestResult, JiraWorklog, MockJira,
};
pub use mock::{jira_key, MockApi, MockScenario};

/// Реализация `FocusApi` в контексте Dioxus (`MockApi` или `HttpApi`).
///
/// У `HttpApi` есть ещё поток событий SSE — тогда клиент живёт на событиях
/// и редком страховочном опросе; у мока событий нет — опрос раз в секунду.
///
/// Настройки плагина Jira — отдельный трейт [`JiraSettingsApi`] (это маршруты
/// плагина, не ядра): у сервиса — тот же `HttpApi`, у мока — [`MockJira`].
#[derive(Clone)]
pub struct ApiHandle(
    pub Rc<dyn FocusApi>,
    Option<HttpApi>,
    Rc<dyn JiraSettingsApi>,
);

impl ApiHandle {
    /// Обёртка над реализацией без событий (мок, тесты); Jira — не настроена
    /// ([`MockJira::unconfigured`]), заменить — [`ApiHandle::with_jira`].
    pub fn new(api: impl FocusApi + 'static) -> Self {
        Self(Rc::new(api), None, Rc::new(MockJira::unconfigured()))
    }

    /// Настоящий сервис: команды по REST, живые обновления по SSE.
    pub fn http(api: HttpApi) -> Self {
        Self(Rc::new(api.clone()), Some(api.clone()), Rc::new(api))
    }

    /// Подменить реализацию настроек Jira.
    pub fn with_jira(mut self, jira: impl JiraSettingsApi + 'static) -> Self {
        self.2 = Rc::new(jira);
        self
    }

    /// Настройки плагина Jira.
    pub fn jira(&self) -> &dyn JiraSettingsApi {
        self.2.as_ref()
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
