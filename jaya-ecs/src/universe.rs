use std::{any::TypeId, hint::unreachable_unchecked, mem::transmute};

use fxhash::FxHashMap;
use parking_lot::RwLock;

use crate::{
    collections::AnyVec,
    component::{Component, IntoComponentIterator},
    entity::{AddEntity, EntityId},
    extract::{
        ComponentModifier, ComponentModifierStager, MultiComponentModifier,
        MultiComponentModifierStager,
    },
};

pub trait Universe: Sync {
    fn queue_remove_entity(&self, id: EntityId);
    fn queue_component_modifier(&self, modifier: ComponentModifier);
    fn queue_multi_component_modifier(&self, modifier: MultiComponentModifier);
}

pub struct StandardUniverse {
    components: RwLock<FxHashMap<TypeId, AnyVec>>,
    mod_queue: ComponentModifierStager,
    multi_mod_queue: MultiComponentModifierStager,
}

impl<T: Component + 'static> IntoComponentIterator<T> for StandardUniverse {
    fn iter_components<'a, F>(&'a self, f: F)
    where
        T: 'a,
        F: Fn(EntityId, &'a T) + Sync,
    {
        self.components.read().get(&TypeId::of::<T>()).map(|bytes| {
            if !bytes.par_iter::<(EntityId, T), _>(|x| unsafe { (f)(x.0, transmute(&x.1)) }) {
                unsafe { unreachable_unchecked() }
            }
        });
    }

    fn iter_component_combinations<'a, F, const N: usize>(&'a self, f: F)
    where
        T: 'a,
        F: Fn([(EntityId, &'a T); N]) + Sync,
    {
        self.components.read().get(&TypeId::of::<T>()).map(|bytes| {
            if !bytes.par_iter_combinations::<(EntityId, T), _, N>(|x| {
                (f)(x.map(|(id, item)| unsafe { transmute((*id, item)) }))
            }) {
                unsafe { unreachable_unchecked() }
            }
        });
    }
}

impl<C1: Component + Send + 'static> AddEntity<(C1,)> for StandardUniverse {
    fn queue_add_entity(&self, id: EntityId, (entity,): (C1,)) {
        let type_id = TypeId::of::<C1>();
        if let Some(vec) = self.components.read().get(&type_id) {
            let _ = vec.push((id, entity));
        } else {
            let vec = AnyVec::new::<(EntityId, C1)>();
            let _ = vec.push((id, entity));
            self.components.write().insert(type_id, vec);
        }
    }
}

impl Universe for StandardUniverse {
    fn queue_remove_entity(&self, _id: EntityId) {
        todo!()
    }

    fn queue_component_modifier(&self, modifier: ComponentModifier) {
        self.mod_queue.queue_modifier(modifier);
    }

    fn queue_multi_component_modifier(&self, modifier: MultiComponentModifier) {
        self.multi_mod_queue.queue_modifier(modifier);
    }
}
