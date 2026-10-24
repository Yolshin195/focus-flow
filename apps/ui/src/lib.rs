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

pub use api::{ApiHandle, FocusApi, HttpApi, JiraSettingsApi, MockApi, MockJira, MockScenario};
pub use platform::{NoopPlatform, Platform, PlatformHandle};
pub use state::UiPreset;

use layout::Layout;
use screens::{Day, Focus, Settings, Tasks};
use state::{AppState, Conn};

const TOKENS_CSS: Asset = asset!("/assets/tokens.css");
/// Без хэша в имени: веб-версия подключает его из `index.html` по постоянному
/// адресу `assets/main.css`, не дожидаясь wasm.
const MAIN_CSS: Asset = asset!(
    "/assets/main.css",
    AssetOptions::builder().with_hash_suffix(false)
);

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
#[component]
pub fn App(#[props(default)] preset: UiPreset) -> Element {
    let platform = use_context::<PlatformHandle>();
    let app = state::use_app_provider(preset);
    let motion = use_motion(app);
    let class = format!(
        "app platform-{}{}",
        platform.0.name(),
        if motion() { " motion" } else { "" }
    );
    rsx! {
        if !platform.0.styles_in_page() {
            Styles {}
        }
        div { class,
            Router::<Route> {}
        }
    }
}

/// Шрифты и стили в `<head>`. Отдельный компонент без props: `App`
/// перерисовывается (класс `motion`), а `<head>` трогать незачем.
#[component]
fn Styles() -> Element {
    rsx! {
        document::Style { {fonts::font_faces()} }
        document::Stylesheet { href: TOKENS_CSS }
        document::Stylesheet { href: MAIN_CSS }
    }
}

/// Ждёт два кадра: первый рисует данные, со второго можно анимировать.
const NEXT_FRAME_JS: &str =
    "await new Promise((r) => requestAnimationFrame(() => requestAnimationFrame(r)));";

/// Класс `motion` на корне включает CSS-переходы. Его нет, пока не пришли
/// данные (или не стало ясно, что связи нет), и ещё один кадр после: при
/// запуске циферблат, полосы и свечение сразу стоят в итоговом состоянии,
/// а не «разгораются» из пустого.
fn use_motion(app: AppState) -> Signal<bool> {
    let mut motion = use_signal(|| false);
    let settled = use_memo(move || {
        app.status.read().is_some()
            || matches!(*app.conn.read(), Conn::Offline | Conn::Unauthorized)
    });
    use_effect(move || {
        if settled() && !*motion.peek() {
            spawn(async move {
                _ = document::eval(NEXT_FRAME_JS).await;
                motion.set(true);
            });
        }
    });
    motion
}
