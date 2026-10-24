//! Платформенные возможности: всё, что `ui` не делает сам.
//!
//! Каждая платформа (`apps/desktop`, `apps/web`, …) кладёт свою реализацию
//! в контекст как [`PlatformHandle`] в своём `main.rs`.

use std::rc::Rc;

/// То, что отличается между платформами.
pub trait Platform {
    /// Имя платформы для CSS-класса `platform-<name>` на корне.
    fn name(&self) -> &'static str;
    /// Системное уведомление (конец работы, конец отдыха).
    fn notify(&self, title: &str, body: &str);
    /// Не давать системе засыпать (`true`) — пока идёт работа.
    fn keep_awake(&self, on: bool);
    /// Базовый адрес сервиса, например `http://127.0.0.1:7878`.
    fn api_base_url(&self) -> String;
}

/// Платформа, которая ничего не умеет, — для тестов и заготовок.
#[derive(Debug, Default, Clone, Copy)]
pub struct NoopPlatform;

impl Platform for NoopPlatform {
    fn name(&self) -> &'static str {
        "desktop"
    }
    fn notify(&self, _title: &str, _body: &str) {}
    fn keep_awake(&self, _on: bool) {}
    fn api_base_url(&self) -> String {
        "http://127.0.0.1:7878".into()
    }
}

/// Платформа в контексте Dioxus.
#[derive(Clone)]
pub struct PlatformHandle(pub Rc<dyn Platform>);

impl PlatformHandle {
    /// Обёртка над реализацией.
    pub fn new(p: impl Platform + 'static) -> Self {
        Self(Rc::new(p))
    }
}
