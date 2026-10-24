//! Модальный диалог: затемнение, карточка, Esc — закрыть.

use dioxus::prelude::*;

/// Диалог. `on_close` — по Esc и клику по фону.
#[component]
pub fn Dialog(title: String, children: Element, on_close: EventHandler<()>) -> Element {
    rsx! {
        div {
            class: "scrim",
            onclick: move |_| on_close.call(()),
            div {
                class: "dialog",
                role: "dialog",
                "aria-modal": "true",
                "aria-label": "{title}",
                tabindex: "-1",
                onclick: move |e| e.stop_propagation(),
                onkeydown: move |e| {
                    if e.key() == Key::Escape {
                        e.stop_propagation();
                        on_close.call(());
                    }
                },
                {children}
            }
        }
    }
}
