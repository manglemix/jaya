use std::fmt::Debug;

use base64::{prelude::BASE64_STANDARD_NO_PAD, Engine};

#[derive(Clone, Copy, Hash, PartialEq, Eq, PartialOrd, Ord, derive_more::From)]
pub struct EntityId(pub(crate) u64);

impl Debug for EntityId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "EntityId({})",
            BASE64_STANDARD_NO_PAD.encode(self.0.to_be_bytes())
        )
    }
}
