//! Поле ввода без привязки `value`.
//!
//! Управляемое поле (`value: "{signal}"` + `oninput`) на desktop теряет
//! символы при быстром наборе: правка DOM от ре-рендера по первому нажатию
//! приходит через IPC уже после второго и затирает его. Здесь текст живёт в
//! DOM, наружу уходит через `on_input`, а сброс (очистить после «Создать»,
//! подставить сохранённые настройки) — пересоздание поля сменой `reset`.

use dioxus::prelude::*;

/// Поле ввода; `initial` — значение при (пере)создании.
#[component]
pub fn TextInput(
    /// Начальное значение.
    #[props(default)]
    initial: String,
    /// Смена значения пересоздаёт поле с `initial`.
    #[props(default)]
    reset: u32,
    /// Вернуть фокус после пересоздания (когда `reset > 0`).
    #[props(default)]
    refocus: bool,
    #[props(default = "text")] kind: &'static str,
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
            input {
                key: "{n}",
                r#type: kind,
                class,
                placeholder: "{placeholder}",
                "aria-label": "{label}",
                initial_value: "{initial}",
                autofocus,
                autocomplete: "off",
                spellcheck: if spellcheck { "true" } else { "false" },
                oninput: move |e| on_input.call(e.value()),
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
