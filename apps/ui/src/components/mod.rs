//! Переиспользуемые компоненты экранов.

mod dialog;
mod icons;
mod norm_bar;
mod picker;
mod ring;
mod text_input;

pub use dialog::Dialog;
pub use icons::Icon;
pub use norm_bar::{NormBar, Segment};
pub use picker::ItemPicker;
pub use ring::{Ring, RingTone};
pub use text_input::TextInput;

use focus_api_types::SessionKind;

/// CSS-модификатор вида сессии: `work`, `rest`, `planning`.
pub fn kind_class(kind: SessionKind) -> &'static str {
    match kind {
        SessionKind::Work => "work",
        SessionKind::Rest => "rest",
        SessionKind::Planning => "planning",
    }
}

/// Название вида сессии.
pub fn kind_label(kind: SessionKind) -> &'static str {
    match kind {
        SessionKind::Work => "Работа",
        SessionKind::Rest => "Отдых",
        SessionKind::Planning => "Планирование",
    }
}
