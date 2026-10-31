//! Typed identifiers.
//!
//! Every entity in a project is addressed by a distinct newtype over `u64`.
//! A single counter in [`crate::model::Project`] hands out ids, which keeps
//! serialised projects stable and makes accidental cross-type use impossible.

use serde::{Deserialize, Serialize};
use std::fmt;

macro_rules! id_type {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(pub u64);

        impl $name {
            /// A sentinel that is never handed out by [`crate::model::Project::alloc_id`].
            pub const NONE: Self = Self(0);

            pub const fn new(raw: u64) -> Self {
                Self(raw)
            }

            pub const fn is_none(self) -> bool {
                self.0 == 0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}", self.0)
            }
        }

        impl From<u64> for $name {
            fn from(value: u64) -> Self {
                Self(value)
            }
        }
    };
}

id_type!(
    /// Identifies an arrangement track.
    TrackId
);
id_type!(
    /// Identifies a clip on a track.
    ClipId
);
id_type!(
    /// Identifies a sequencer pattern owned by a track.
    PatternId
);
id_type!(
    /// Identifies a single note inside a pattern.
    NoteId
);
id_type!(
    /// Identifies a hosted plugin instance (VST2/VST3 insert or instrument).
    PluginInstanceId
);
