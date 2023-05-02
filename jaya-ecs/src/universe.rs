use crate::{entity::EntityId, extract::{ComponentModifier, MultiComponentModifier}};

pub trait Universe: Sync {
    fn queue_remove_entity(&self, id: EntityId);
    fn queue_component_modifier(&self, modifier: ComponentModifier);
    fn queue_multi_component_modifier(&self, modifier: MultiComponentModifier);
}