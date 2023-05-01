use std::fmt::{Debug};

use base64::{prelude::BASE64_STANDARD_NO_PAD, Engine};
use rand::Rng;

use crate::component::{Component, ComponentContainer};

#[derive(Clone, Copy, Hash, PartialEq, Eq, PartialOrd, Ord, derive_more::From)]
pub struct EntityId(usize);


impl EntityId {
    pub fn rand() -> Self {
        Self(rand::random())
    }
    pub fn rand_with_rng<R: Rng + ?Sized>(rng: &mut R) -> Self {
        Self(rng.gen())
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


pub trait RemoveEntity<P> {
    fn queue_remove_entity(&self, id: EntityId);
}


pub trait AddEntity<T> {
    fn queue_add_entity(&self, id: EntityId, entity: T);
}


impl<'a, U, C1> AddEntity<(C1,)> for U
where
    C1: Component + 'a,
    U: ComponentContainer<'a, C1>
{
    fn queue_add_entity(&self, id: EntityId, (c1,): (C1,)) {
        self.queue_add_component(id, c1);
    }
}


impl<'a, U, C1, C2> AddEntity<(C1, C2)> for U
where
    C1: Component + 'a,
    C2: Component + 'a,
    U: ComponentContainer<'a, C1> + ComponentContainer<'a, C2>
{
    fn queue_add_entity(&self, id: EntityId, (c1, c2): (C1, C2)) {
        self.queue_add_component(id, c1);
        self.queue_add_component(id, c2);
    }
}

impl<'a, U, C1> RemoveEntity<(C1,)> for U
where
    C1: Component + 'a,
    U: ComponentContainer<'a, C1>
{
    fn queue_remove_entity(&self, id: EntityId) {
        self.queue_remove_component(id);
    }
}

impl<'a, U, C1, C2> RemoveEntity<(C1, C2)> for U
where
    C1: Component + 'a,
    C2: Component + 'a,
    U: ComponentContainer<'a, C1> + ComponentContainer<'a, C2>
{
    fn queue_remove_entity(&self, id: EntityId) {
        ComponentContainer::<C1>::queue_remove_component(self, id);
        ComponentContainer::<C2>::queue_remove_component(self, id);
    }
}


impl Debug for EntityId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "EntityId({})", BASE64_STANDARD_NO_PAD.encode(self.0.to_be_bytes()))
    }
}

// pub trait StandardEntityContainer<T>
// {
//     fn get_entities_map(&self) -> &FxHashMap<EntityId, T>;
// }

// impl<'a, C, C1, C2> ComponentContainer<'a, C1> for C
// where
//     C: StandardEntityContainer<(C1, C2)>
// {

// }