//! Многострочное поле без привязки `value` — как [`super::TextInput`].
//!
//! Текст живёт в DOM, наружу уходит через `on_input`, сброс — пересоздание
//! поля сменой `reset`. Высота растёт по тексту: CSS `field-sizing: content`,
//! где он есть, иначе — по `scrollHeight` после каждого ввода.

use dioxus::prelude::*;

/// Подогнать высоту поля в фокусе под текст (для движков без `field-sizing`).
const AUTOSIZE_JS: &str = r#"const el = document.activeElement;
if (el && el.tagName === "TEXTAREA" && !CSS.supports("field-sizing", "content")) {
  el.style.height = "auto";
  el.style.height = el.scrollHeight + "px";
}"#;

/// Многострочное поле ввода.
#[component]
pub fn TextArea(
    /// Смена значения пересоздаёт поле пустым.
    #[props(default)]
    reset: u32,
    /// Вернуть фокус после пересоздания (когда `reset > 0`).
    #[props(default)]
    refocus: bool,
    #[props(default)] class: &'static str,
    #[props(default)] placeholder: String,
    #[props(default)] label: String,
    #[props(default)] autofocus: bool,
    #[props(default = true)] spellcheck: bool,
    #[props(default)] on_input: EventHandler<String>,
    #[props(default)] on_keydown: EventHandler<KeyboardEvent>,
    #[props(default)] on_blur: EventHandler<()>,
) -> Element {
    rsx! {
        for n in [reset] {
            textarea {
                key: "{n}",
                class,
                rows: "1",
                placeholder: "{placeholder}",
                "aria-label": "{label}",
                autofocus,
                autocomplete: "off",
                spellcheck: if spellcheck { "true" } else { "false" },
                oninput: move |e| {
                    on_input.call(e.value());
                    document::eval(AUTOSIZE_JS);
                },
                onkeydown: move |e| on_keydown.call(e),
                onblur: move |_| on_blur.call(()),
                onmounted: move |e| async move {
                    if refocus && n > 0 {
                        let _ = e.set_focus(true).await;
                    }
                },
            }
        }
    }
}
