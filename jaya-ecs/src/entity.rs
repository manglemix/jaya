use std::fmt::Debug;

use base64::{prelude::BASE64_STANDARD_NO_PAD, Engine};

#[derive(Clone, Copy, Hash, PartialEq, Eq, PartialOrd, Ord, derive_more::From)]
pub struct EntityId(pub(crate) usize);

impl EntityId {
    pub fn rand() -> Self {
        Self(fastrand::usize(..))
    }
    //     pub fn rand_unique<T>(map: &EntityMap<T>) -> Self {
    //         let mut rng = thread_rng();
    //         loop {
    //             let id = Self(rng.gen());
    //             if !map.contains_key(&id) {
    //                 break id;
    //             }
    //         }
    //     }
    //     pub fn rand_unique_with_rng<R: Rng + ?Sized, T>(rng: &mut R, map: &EntityMap<T>) -> Self {
    //         loop {
    //             let id = Self(rng.gen());
    //             if !map.contains_key(&id) {
    //                 break id;
    //             }
    //         }
    //     }
}

impl Debug for EntityId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "EntityId({})",
            BASE64_STANDARD_NO_PAD.encode(self.0.to_be_bytes())
        )
    }
}

pub trait RemoveEntity<P> {
    fn queue_remove_entity(&self, id: EntityId);
}

pub trait AddEntity<T> {
    fn queue_add_entity(&self, id: EntityId, entity: T);
}
