use crate::{entity::EntityId};


pub trait Component: Sync {}

pub trait IntoComponentIterator<T: Component> {
    fn iter_components<'a, F>(&'a self, f: F)
    where
        T: 'a,
        F: Fn(EntityId, &'a T) + Sync;
    fn iter_component_combinations<'a, F, const N: usize>(&'a self, f: F)
    where
        T: 'a,
        F: Fn([(EntityId, &'a T); N]) + Sync;
}