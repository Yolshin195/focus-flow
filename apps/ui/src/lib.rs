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

use dioxus::prelude::*;

pub use api::{ApiHandle, FocusApi, HttpApi, MockApi, MockScenario};
pub use platform::{NoopPlatform, Platform, PlatformHandle};
pub use state::UiPreset;

use layout::Layout;
use screens::{Day, Focus, Tasks};

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
}

/// Корневой компонент. Ждёт в контексте [`PlatformHandle`] и [`ApiHandle`].
#[component]
pub fn App(#[props(default)] preset: UiPreset) -> Element {
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
    }
}
