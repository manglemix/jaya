use rayon::prelude::ParallelIterator;

use crate::{entity::EntityId, extract::{ComponentModifier, MultiComponentModifier}};


pub trait Component: Send + Sync {}

pub trait ComponentContainer<T>
where
    T: Component
{
    // type Iterator: ParallelIterator<Item = (EntityId, &'a T)>;

    fn queue_add_component(&self, id: EntityId, component: T);
    fn queue_remove_component(&self, id: EntityId);

    fn iter_components(&self, f: impl Fn(&T));
}

pub trait ComponentModifierContainer {
    fn queue_component_modifier(&self, modifier: ComponentModifier);
    fn queue_multi_component_modifier(&self, modifier: MultiComponentModifier);
}