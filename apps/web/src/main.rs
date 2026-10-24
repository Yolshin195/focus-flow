//! Focus Flow в браузере (заготовка): `MockApi` и платформа без уведомлений.

use dioxus::prelude::*;
use ui::{ApiHandle, App, MockApi, MockScenario, Platform, PlatformHandle, UiPreset};

const FAVICON: Asset = asset!("/assets/favicon.ico");

struct WebPlatform;

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
}

#[component]
fn Root() -> Element {
    use_context_provider(|| PlatformHandle::new(WebPlatform));
    use_context_provider(|| ApiHandle::new(MockApi::demo(MockScenario::Idle, 1.0)));
    rsx! {
        document::Link { rel: "icon", href: FAVICON }
        App { preset: UiPreset::None }
    }
}

fn main() {
    dioxus::launch(Root);
}
