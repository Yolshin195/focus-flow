//! Focus Flow для desktop: реализация `Platform`, выбор `FocusApi`, запуск `ui::App`.
//!
//! Переменные окружения для разработки и демо:
//! - `FOCUS_MOCK_SCENARIO` — `idle` (по умолчанию) | `work` | `rest` | `planning` |
//!   `empty` | `picker` | `interrupt` | `done-choice` | `tasks` | `day`;
//! - `FOCUS_MOCK_SPEED` — во сколько раз время мока быстрее реального (по умолчанию 1);
//! - `FOCUS_WINDOW_SIZE` — размер окна `ШxВ` (по умолчанию `1180x780`).

use dioxus::desktop::{Config, LogicalSize, WindowBuilder};
use dioxus::prelude::*;
use ui::{ApiHandle, App, MockApi, MockScenario, Platform, PlatformHandle, UiPreset};

/// Фон окна до первой отрисовки — совпадает с `--bg` в `tokens.css`.
const BG: (u8, u8, u8) = (0x0f, 0x11, 0x15);

struct DesktopPlatform;

impl Platform for DesktopPlatform {
    fn name(&self) -> &'static str {
        "desktop"
    }

    fn notify(&self, title: &str, body: &str) {
        let shown = notify_rust::Notification::new()
            .appname("Focus Flow")
            .summary(title)
            .body(body)
            .show();
        if let Err(e) = shown {
            eprintln!("уведомление не показано: {e}");
        }
    }

    fn keep_awake(&self, _on: bool) {
        // TODO: системный wake lock (inhibit через D-Bus / caffeinate / SetThreadExecutionState).
    }

    fn api_base_url(&self) -> String {
        std::env::var("FOCUS_API_URL").unwrap_or_else(|_| "http://127.0.0.1:7878".into())
    }
}

fn scenario() -> String {
    std::env::var("FOCUS_MOCK_SCENARIO").unwrap_or_default()
}

#[component]
fn Root() -> Element {
    use_context_provider(|| PlatformHandle::new(DesktopPlatform));
    use_context_provider(|| {
        let speed = std::env::var("FOCUS_MOCK_SPEED")
            .ok()
            .and_then(|s| s.parse::<f64>().ok())
            .unwrap_or(1.0);
        ApiHandle::new(MockApi::demo(MockScenario::parse(&scenario()), speed))
    });
    let preset = UiPreset::parse(&scenario());
    rsx! { App { preset } }
}

/// Размер окна из `FOCUS_WINDOW_SIZE` (`960x640`) или по умолчанию.
fn window_size() -> (f64, f64) {
    std::env::var("FOCUS_WINDOW_SIZE")
        .ok()
        .and_then(|v| {
            let (w, h) = v.split_once('x')?;
            Some((w.trim().parse().ok()?, h.trim().parse().ok()?))
        })
        .unwrap_or((1180.0, 780.0))
}

fn main() {
    let (r, g, b) = BG;
    let (w, h) = window_size();
    let window = WindowBuilder::new()
        .with_title("Focus Flow")
        .with_inner_size(LogicalSize::new(w, h))
        .with_min_inner_size(LogicalSize::new(960.0, 640.0));
    let head = format!(
        "<style>html,body{{margin:0;background:rgb({r},{g},{b});color-scheme:dark;}}</style>"
    );
    let cfg = Config::new()
        .with_window(window)
        .with_menu(None)
        .with_background_color((r, g, b, 255))
        .with_custom_head(head);
    dioxus::LaunchBuilder::desktop().with_cfg(cfg).launch(Root);
}
