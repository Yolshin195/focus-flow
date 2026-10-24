//! Общий интерфейс Focus Flow: маршруты, экраны, компоненты, тема.
//!
//! Платформа (desktop, web, …) кладёт в контекст [`PlatformHandle`] и
//! [`ApiHandle`] и рендерит [`App`].

pub mod api;
mod components;
pub mod fmt;
mod fonts;
mod layout;
pub mod platform;
mod screens;
pub mod state;
pub mod timer;

use dioxus::prelude::*;

pub use api::{ApiHandle, FocusApi, HttpApi, JiraSettingsApi, MockApi, MockJira, MockScenario};
pub use platform::{NoopPlatform, Platform, PlatformHandle};
pub use state::{Reveal, UiPreset};
pub use timer::{use_status_line, use_timer_view, Indicator, StatusLine, TimerView};

use layout::Layout;
use screens::{Day, Focus, Settings, Tasks};

const TOKENS_CSS: Asset = asset!("/assets/tokens.css");
const MAIN_CSS: Asset = asset!("/assets/main.css");

/// Маршруты приложения.
#[derive(Debug, Clone, Routable, PartialEq)]
#[rustfmt::skip]
pub enum Route {
    #[layout(Layout)]
        #[route("/")]
        Focus {},
        #[route("/tasks")]
        Tasks {},
        #[route("/day")]
        Day {},
        #[route("/settings")]
        Settings {},
}

/// Корневой компонент. Ждёт в контексте [`PlatformHandle`] и [`ApiHandle`].
///
/// `children` — платформенные компоненты без разметки (трей на desktop):
/// они видят состояние приложения (`state::use_app`), но не роутер; открыть
/// экран им помогает [`state::AppState::reveal`].
#[component]
pub fn App(#[props(default)] preset: UiPreset, children: Element) -> Element {
    let platform = use_context::<PlatformHandle>();
    state::use_app_provider(preset);
    let class = format!("app platform-{}", platform.0.name());
    rsx! {
        document::Style { {fonts::font_faces()} }
        document::Stylesheet { href: TOKENS_CSS }
        document::Stylesheet { href: MAIN_CSS }
        div { class,
            Router::<Route> {}
        }
        {children}
    }
}
