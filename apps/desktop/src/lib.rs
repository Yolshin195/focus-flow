//! Focus Flow для desktop: реализация `Platform`, выбор `FocusApi`, запуск `ui::App`.
//!
//! Библиотека, чтобы окно открывал и `focus-flow ui` (сервис в том же
//! процессе), и `dx serve` (`src/main.rs`). Вход — [`launch`].
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
//!   `empty` | `picker` | `interrupt` | `done-choice` | `tasks` | `day` | `settings`;
//! - `FOCUS_MOCK_JIRA=off` — в моке Jira не настроена (по умолчанию «подключена»);
//! - `FOCUS_MOCK_SPEED` — во сколько раз время мока быстрее реального (по умолчанию 1);
//! - `FOCUS_WINDOW_SIZE` — размер окна `ШxВ` (по умолчанию `1180x780`).
//!
//! Трей / строка меню — [`tray`]: время текущей фазы рядом с иконкой и
//! команды по фазе. Пока трей есть, закрытие окна его только прячет.

use dioxus::desktop::tao::event::Event;
use dioxus::desktop::{Config, LogicalSize, WindowBuilder};
use dioxus::prelude::*;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use ui::{
    ApiHandle, App, HttpApi, MockApi, MockJira, MockScenario, Platform, PlatformHandle, UiPreset,
};

mod tray;

/// Адрес сервиса по умолчанию — как у `focus-flow serve`.
const DEFAULT_ADDR: &str = "127.0.0.1:7878";

/// Фон окна до первой отрисовки — совпадает с `--bg` в `tokens.css`.
const BG: (u8, u8, u8) = (0x0f, 0x11, 0x15);

/// Как подключаться к сервису. Пустые поля — из окружения
/// (`FOCUS_ADDR`, `FOCUS_TOKEN`, файл `token`).
#[derive(Debug, Clone, Default)]
pub struct Options {
    /// Адрес сервиса (`127.0.0.1:7878` или `http://…`).
    pub addr: Option<String>,
    /// Токен доступа (например, его только что прочитал `focus-flow ui`).
    pub token: Option<String>,
}

impl Options {
    /// Всё из окружения.
    pub fn from_env() -> Self {
        Self::default()
    }
}

static OPTIONS: OnceLock<Options> = OnceLock::new();

fn options() -> &'static Options {
    OPTIONS.get_or_init(Options::default)
}

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
        let addr = options()
            .addr
            .clone()
            .or_else(|| env("FOCUS_ADDR"))
            .unwrap_or_else(|| DEFAULT_ADDR.into());
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

/// Токен: из [`Options`], `FOCUS_TOKEN` или содержимое файла токена (на момент вызова).
fn read_token() -> Option<String> {
    if let Some(t) = options().token.clone() {
        return Some(t);
    }
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

/// Применять правки DOM сразу, а не в `requestAnimationFrame`. Dioxus ждёт
/// от страницы подтверждения каждой пачки правок и до него не крутит задачи,
/// а у скрытого или свёрнутого окна rAF не вызывается — вставало бы всё:
/// SSE, ход таймера, трей. Так Dioxus работает с окнами, созданными
/// невидимыми (`headless`): на macOS и Windows — всегда (окно показывается
/// после первой отрисовки), на Linux — нет, там это и включаем.
fn apply_edits_when_hidden() {
    let script = "window.interpreter.headless = true;";
    if let Err(e) = dioxus::desktop::window().webview.evaluate_script(script) {
        eprintln!("не удалось включить правки без rAF: {e}");
    }
}

#[component]
fn Root() -> Element {
    use_hook(apply_edits_when_hidden);
    use_context_provider(|| PlatformHandle::new(DesktopPlatform));
    use_context_provider(|| {
        if !mock_mode() {
            let base = DesktopPlatform.api_base_url();
            return ApiHandle::http(HttpApi::with_token_source(base, read_token));
        }
        let speed = env("FOCUS_MOCK_SPEED")
            .and_then(|s| s.parse::<f64>().ok())
            .unwrap_or(1.0);
        // В демо Jira «подключена»; `FOCUS_MOCK_JIRA=off` — как без настройки.
        let jira = if env("FOCUS_MOCK_JIRA").as_deref() == Some("off") {
            MockJira::unconfigured()
        } else {
            MockJira::demo()
        };
        ApiHandle::new(MockApi::demo(MockScenario::parse(&scenario()), speed)).with_jira(jira)
    });
    let preset = UiPreset::parse(&scenario());
    rsx! {
        App { preset,
            tray::Tray {}
        }
    }
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

/// Подготовить окружение процесса до запуска потоков: если приложение
/// собрано не через `dx` (обычный `cargo build`/`cargo run`/`cargo install`)
/// и рядом с бинарником нет каталога `assets`, `asset!()` берёт файлы из
/// исходников (`apps/ui/assets`). Признак такого режима для Dioxus —
/// `CARGO_MANIFEST_DIR`; `cargo run` задаёт его сам, здесь — для запуска
/// без cargo. Вызывать первой строкой `main`, пока в процессе один поток.
pub fn prepare_env() {
    let set = |k: &str| std::env::var_os(k).is_some_and(|v| !v.is_empty());
    if set("DIOXUS_CLI_ENABLED") || set("CARGO_MANIFEST_DIR") {
        return;
    }
    let bundled = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|d| d.join("assets")))
        .is_some_and(|d| d.is_dir());
    let sources = Path::new(env!("CARGO_MANIFEST_DIR"));
    if !bundled && sources.join("../ui/assets").is_dir() {
        // Один поток (см. документацию функции) — гонки с чтением окружения нет.
        std::env::set_var("CARGO_MANIFEST_DIR", sources);
    }
}

/// Открыть окно и крутить цикл событий до выхода. `on_exit` вызывается
/// один раз, когда цикл событий завершается («Выход» в трее, ⌘Q, Ctrl+C,
/// SIGTERM; без трея — и закрытие окна), — до выхода из процесса: здесь
/// `focus-flow ui` останавливает свой сервис. С треем закрытие окна его
/// только прячет. Сама функция не возвращается.
pub fn launch(opts: Options, on_exit: impl FnOnce() + 'static) {
    // Повторная установка невозможна (окно одно на процесс) — игнорируем.
    let _ = OPTIONS.set(opts);
    let (r, g, b) = BG;
    let (w, h) = window_size();
    let window = WindowBuilder::new()
        .with_title("Focus Flow")
        .with_inner_size(LogicalSize::new(w, h))
        .with_min_inner_size(LogicalSize::new(960.0, 640.0));
    let head = format!(
        "<style>html,body{{margin:0;background:rgb({r},{g},{b});color-scheme:dark;}}</style>"
    );
    let mut on_exit = Some(on_exit);
    let cfg = Config::new().with_window(window);
    // На macOS меню оставляем стандартным: без меню Edit в WKWebView не
    // работают ⌘C/⌘V/⌘X/⌘A (не вставить токен Jira или ссылку), а без
    // меню приложения — ⌘Q. На Linux и Windows меню в окне не нужно.
    #[cfg(not(target_os = "macos"))]
    let cfg = cfg.with_menu(None);
    let cfg = cfg
        // Левый клик по трею обрабатывает `tray` (окно + экран «Фокус»).
        .with_tray_icon_show_window_on_click(false)
        .with_background_color((r, g, b, 255))
        .with_custom_head(head)
        .with_custom_event_handler(move |ev, _| {
            if matches!(ev, Event::LoopDestroyed) {
                if let Some(f) = on_exit.take() {
                    f();
                }
            }
        });
    dioxus::LaunchBuilder::desktop().with_cfg(cfg).launch(Root);
}
