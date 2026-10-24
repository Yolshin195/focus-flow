//! Focus Flow для desktop: реализация `Platform`, выбор `FocusApi`, запуск `ui::App`.
//!
//! По умолчанию — настоящий сервис (`HttpApi`):
//! - `FOCUS_ADDR` — адрес сервиса (`127.0.0.1:7878` по умолчанию; можно с `http://`);
//! - `FOCUS_TOKEN` — токен; без него читается файл `token` из `FOCUS_DATA_DIR`
//!   или из каталога данных сервиса (Linux `~/.local/share/focus-flow/token`,
//!   macOS `~/Library/Application Support/dev.FocusFlow.focus-flow/token`).
//!   Файл перечитывается, если сервис ответил `401` (например, его пересоздали).
//!
//! `FOCUS_MOCK=1` (или заданный `FOCUS_MOCK_SCENARIO`) — `MockApi` для демо:
//! - `FOCUS_MOCK_SCENARIO` — `idle` (по умолчанию) | `work` | `rest` | `planning` |
//!   `empty` | `picker` | `interrupt` | `done-choice` | `tasks` | `day`;
//! - `FOCUS_MOCK_SPEED` — во сколько раз время мока быстрее реального (по умолчанию 1);
//! - `FOCUS_WINDOW_SIZE` — размер окна `ШxВ` (по умолчанию `1180x780`).

use dioxus::desktop::{Config, LogicalSize, WindowBuilder};
use dioxus::prelude::*;
use std::path::PathBuf;

use ui::{ApiHandle, App, HttpApi, MockApi, MockScenario, Platform, PlatformHandle, UiPreset};

/// Адрес сервиса по умолчанию — как у `focus-flow serve`.
const DEFAULT_ADDR: &str = "127.0.0.1:7878";

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
        let addr = env("FOCUS_ADDR").unwrap_or_else(|| DEFAULT_ADDR.into());
        if addr.contains("://") {
            addr
        } else {
            format!("http://{addr}")
        }
    }
}

/// Непустая переменная окружения.
fn env(key: &str) -> Option<String> {
    std::env::var(key)
        .ok()
        .map(|v| v.trim().to_owned())
        .filter(|v| !v.is_empty())
}

fn scenario() -> String {
    env("FOCUS_MOCK_SCENARIO").unwrap_or_default()
}

/// Режим демо: `FOCUS_MOCK=1` или задан сценарий мока.
fn mock_mode() -> bool {
    env("FOCUS_MOCK").is_some_and(|v| v != "0") || env("FOCUS_MOCK_SCENARIO").is_some()
}

/// Файл токена сервиса: `$FOCUS_DATA_DIR/token` или каталог данных
/// `ProjectDirs("dev", "FocusFlow", "focus-flow")` — тот же, что у сервиса.
fn token_path() -> Option<PathBuf> {
    let dir = match env("FOCUS_DATA_DIR") {
        Some(d) => PathBuf::from(d),
        None => directories::ProjectDirs::from("dev", "FocusFlow", "focus-flow")?
            .data_dir()
            .to_path_buf(),
    };
    Some(dir.join("token"))
}

/// Токен: `FOCUS_TOKEN` или содержимое файла токена (на момент вызова).
fn read_token() -> Option<String> {
    if let Some(t) = env("FOCUS_TOKEN") {
        return Some(t);
    }
    let path = token_path()?;
    match std::fs::read_to_string(&path) {
        Ok(s) => Some(s.trim().to_owned()).filter(|t| !t.is_empty()),
        Err(e) => {
            eprintln!("токен не прочитан из {}: {e}", path.display());
            None
        }
    }
}

#[component]
fn Root() -> Element {
    use_context_provider(|| PlatformHandle::new(DesktopPlatform));
    use_context_provider(|| {
        if !mock_mode() {
            let base = DesktopPlatform.api_base_url();
            return ApiHandle::http(HttpApi::with_token_source(base, read_token));
        }
        let speed = env("FOCUS_MOCK_SPEED")
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
