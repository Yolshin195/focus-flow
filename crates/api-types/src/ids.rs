//! Идентификаторы сущностей — newtype поверх [`Uuid`], чтобы не перепутать
//! id задачи с id сессии. В JSON — обычная строка UUID.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

macro_rules! id_type {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(pub Uuid);

        impl $name {
            /// Оборачивает готовый [`Uuid`].
            pub const fn from_uuid(uuid: Uuid) -> Self {
                Self(uuid)
            }

            /// Внутренний [`Uuid`].
            pub const fn as_uuid(&self) -> Uuid {
                self.0
            }
        }

        impl From<Uuid> for $name {
            fn from(uuid: Uuid) -> Self {
                Self(uuid)
            }
        }

        impl From<$name> for Uuid {
            fn from(id: $name) -> Self {
                id.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.fmt(f)
            }
        }

        impl FromStr for $name {
            type Err = uuid::Error;

            fn from_str(s: &str) -> Result<Self, Self::Err> {
                Uuid::parse_str(s).map(Self)
            }
        }
    };
}

id_type!(
    /// Идентификатор задачи или пункта (узла дерева задач).
    TaskId
);
id_type!(
    /// Идентификатор сессии (`work`, `rest`, `planning`).
    SessionId
);
id_type!(
    /// Идентификатор рабочего дня.
    DayId
);
