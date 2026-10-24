//! Многострочное поле без привязки `value` — как [`super::TextInput`].
//!
//! Текст живёт в DOM, наружу уходит через `on_input`, сброс — пересоздание
//! поля сменой `reset`. Высота растёт по тексту: CSS `field-sizing: content`,
//! где он есть, иначе — по `scrollHeight` после каждого ввода.

use dioxus::prelude::*;

/// Подогнать высоту полей под текст (для движков без `field-sizing`):
/// после ввода — поле в фокусе, после создания — все (в них `initial`).
const FIT_FOCUSED_JS: &str = r#"if (!CSS.supports("field-sizing", "content")) {
  const el = document.activeElement;
  if (el && el.tagName === "TEXTAREA") { el.style.height = "auto"; el.style.height = el.scrollHeight + "px"; }
}"#;
const FIT_ALL_JS: &str = r#"if (!CSS.supports("field-sizing", "content")) {
  for (const el of document.querySelectorAll("textarea")) { el.style.height = "auto"; el.style.height = el.scrollHeight + "px"; }
}"#;

/// Многострочное поле ввода; `initial` — значение при (пере)создании.
#[component]
pub fn TextArea(
    /// Начальное значение.
    #[props(default)]
    initial: String,
    /// Смена значения пересоздаёт поле с `initial`.
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
                initial_value: "{initial}",
                autofocus,
                autocomplete: "off",
                spellcheck: if spellcheck { "true" } else { "false" },
                oninput: move |e| {
                    on_input.call(e.value());
                    document::eval(FIT_FOCUSED_JS);
                },
                onkeydown: move |e| on_keydown.call(e),
                onblur: move |_| on_blur.call(()),
                onmounted: move |e| async move {
                    document::eval(FIT_ALL_JS);
                    if refocus && n > 0 {
                        let _ = e.set_focus(true).await;
                    }
                },
            }
        }
    }
}
