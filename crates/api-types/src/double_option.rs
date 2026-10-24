//! Сериализация `Option<Option<T>>` для PATCH: поле отсутствует — `None`,
//! `null` — `Some(None)`, значение — `Some(Some(v))`. Используется вместе с
//! `#[serde(default, skip_serializing_if = "Option::is_none")]`.

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// Сериализует внутренний `Option<T>` (`None` внутри → `null`).
pub fn serialize<T, S>(value: &Option<Option<T>>, serializer: S) -> Result<S::Ok, S::Error>
where
    T: Serialize,
    S: Serializer,
{
    match value {
        Some(inner) => inner.serialize(serializer),
        None => serializer.serialize_none(),
    }
}

/// Читает присутствующее поле: `null` → `Some(None)`.
pub fn deserialize<'de, T, D>(deserializer: D) -> Result<Option<Option<T>>, D::Error>
where
    T: Deserialize<'de>,
    D: Deserializer<'de>,
{
    Option::<T>::deserialize(deserializer).map(Some)
}
