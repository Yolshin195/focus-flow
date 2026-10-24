//! Focus Flow в браузере: `MockApi` и платформа без уведомлений.
//!
//! Библиотека, чтобы тест `tests/skeleton.rs` сверял статичный каркас в
//! `index.html` с первым кадром [`Root`]. Точка входа для `dx` — `src/main.rs`.

use dioxus::prelude::*;
use ui::{ApiHandle, App, MockApi, MockScenario, Platform, PlatformHandle, UiPreset};

/// Убрать статичный каркас из `index.html`. Dioxus не заменяет содержимое
/// `#main`, а дописывает своё дерево следом. Код выполняется синхронно в
/// первом рендере — до того, как Dioxus вставит первый кадр, — так что
/// браузер не успевает нарисовать ни пустой экран, ни оба дерева сразу.
const REMOVE_SKELETON_JS: &str = r#"document.getElementById("boot")?.remove();"#;

/// Платформа браузера.
pub struct WebPlatform;

impl Platform for WebPlatform {
    fn name(&self) -> &'static str {
        "web"
    }
    fn notify(&self, _title: &str, _body: &str) {
        // TODO: Notification API браузера.
    }
    fn keep_awake(&self, _on: bool) {
        // TODO: Screen Wake Lock API.
    }
    fn api_base_url(&self) -> String {
        "http://127.0.0.1:7878".into()
    }
    /// Стили, токены и `@font-face` подключает `index.html`.
    fn styles_in_page(&self) -> bool {
        true
    }
}

/// Корень веб-версии: демо на `MockApi`.
#[component]
pub fn Root() -> Element {
    use_hook(|| {
        document::eval(REMOVE_SKELETON_JS);
    });
    use_context_provider(|| PlatformHandle::new(WebPlatform));
    use_context_provider(|| ApiHandle::new(MockApi::demo(MockScenario::Idle, 1.0)));
    rsx! {
        App { preset: UiPreset::None }
    }
}
