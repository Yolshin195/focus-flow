//! Сигналы `AppState` можно читать и писать из корневого scope.
//!
//! Команды (`state::run`) выполняются в `spawn_forever`, то есть в корневом
//! scope, а состояние создаёт `ui::App` — его потомок. Если сигналами владеет
//! `App`, Dioxus на каждое чтение и запись из корня пишет в лог
//! предупреждение `copy_value_hoisted` (в debug-сборке).

use std::cell::Cell;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use dioxus::core::NoOpMutations;
use dioxus::history::{provide_history_context, MemoryHistory};
use dioxus::logger::tracing::{self, field, span, Event, Metadata, Subscriber};
use dioxus::prelude::*;
use focus_api_types::chrono::Utc;
use ui::state::{run, use_api, use_app, Command};
use ui::{ApiHandle, MockApi, NoopPlatform, PlatformHandle};

thread_local! {
    static DONE: Cell<bool> = const { Cell::new(false) };
}

/// Подписчик `tracing`: запоминает цель и текст предупреждений и ошибок.
#[derive(Clone, Default)]
struct Capture(Arc<Mutex<Vec<String>>>);

impl Capture {
    fn lines(&self) -> Vec<String> {
        self.0.lock().map(|l| l.clone()).unwrap_or_default()
    }
}

struct Message(String);

impl field::Visit for Message {
    fn record_debug(&mut self, f: &field::Field, v: &dyn std::fmt::Debug) {
        if f.name() == "message" {
            self.0 = format!("{v:?}");
        }
    }
}

impl Subscriber for Capture {
    fn enabled(&self, m: &Metadata<'_>) -> bool {
        *m.level() <= tracing::Level::WARN
    }
    fn new_span(&self, _: &span::Attributes<'_>) -> span::Id {
        span::Id::from_u64(1)
    }
    fn record(&self, _: &span::Id, _: &span::Record<'_>) {}
    fn record_follows_from(&self, _: &span::Id, _: &span::Id) {}
    fn event(&self, e: &Event<'_>) {
        let mut m = Message(String::new());
        e.record(&mut m);
        if let Ok(mut l) = self.0.lock() {
            l.push(format!("{}: {}", e.metadata().target(), m.0));
        }
    }
    fn enter(&self, _: &span::Id) {}
    fn exit(&self, _: &span::Id) {}
}

#[component]
fn Root() -> Element {
    use_context_provider(|| PlatformHandle::new(NoopPlatform));
    use_context_provider(|| ApiHandle::new(MockApi::frozen(Utc::now())));
    use_hook(|| provide_history_context(Rc::new(MemoryHistory::default())));
    rsx! {
        ui::App { Trigger {} }
    }
}

/// Сразу запускает команду — как кнопка или пункт трея.
#[component]
fn Trigger() -> Element {
    let api = use_api();
    let app = use_app();
    use_hook(move || run(api, app, Command::StartPlanning, || DONE.set(true)));
    rsx! {}
}

#[tokio::test(flavor = "current_thread")]
async fn command_in_root_scope_does_not_hoist_signals() {
    let log = Capture::default();
    let _guard = tracing::subscriber::set_default(log.clone());

    let mut dom = VirtualDom::new(Root);
    dom.rebuild_in_place();
    // Команда закончилась и после неё нет срочной работы (опрос мока и
    // локальные часы ждут по секунде) — всё, что она пишет, уже записано.
    let mut idle_after_done = 0;
    for _ in 0..200 {
        let work = tokio::time::timeout(Duration::from_millis(20), dom.wait_for_work()).await;
        dom.render_immediate(&mut NoOpMutations);
        if DONE.get() {
            idle_after_done = if work.is_err() {
                idle_after_done + 1
            } else {
                0
            };
            if idle_after_done >= 3 {
                break;
            }
        }
    }
    assert!(DONE.get(), "команда не выполнилась");

    let hoisted: Vec<String> = log
        .lines()
        .into_iter()
        .filter(|l| l.contains("copy_value_hoisted"))
        .collect();
    assert!(hoisted.is_empty(), "{}", hoisted.join("\n\n"));
}
