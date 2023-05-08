use std::{any::{TypeId}, hint::unreachable_unchecked, mem::transmute};

use fxhash::FxHashMap;
use parking_lot::RwLock;

use crate::{entity::{EntityId, AddEntity}, extract::{ComponentModifier, MultiComponentModifier}, component::{IntoComponentIterator, Component}, collections::AnyVec};

pub trait Universe: Sync {
    fn queue_remove_entity(&self, id: EntityId);
    fn queue_component_modifier(&self, modifier: ComponentModifier);
    fn queue_multi_component_modifier(&self, modifier: MultiComponentModifier);
}


pub struct StandardUniverse {
    components: RwLock<FxHashMap<TypeId, AnyVec>>,
    // add_entity_queue: SegQueue<>
}


// struct RawComponent {
//     id: EntityId,
//     bytes: 
// }


impl<T: Component + 'static> IntoComponentIterator<T> for StandardUniverse {
    fn iter_components<'a, F>(&'a self, f: F)
    where
        T: 'a,
        F: Fn(EntityId, &'a T) + Sync
    {
        self.components
            .read()
            .get(&TypeId::of::<T>())
            .map(|bytes| {
                if !bytes
                    .par_iter::<(EntityId, T), _>(|x| unsafe {
                        (f)(x.0, transmute(&x.1))
                    }) {
                        unsafe { unreachable_unchecked() }
                    }
            });
    }

    fn iter_component_combinations<'a, F, const N: usize>(&'a self, f: F)
    where
        T: 'a,
        F: Fn([(EntityId, &'a T); N]) + Sync
    {
        self.components
            .read()
            .get(&TypeId::of::<T>())
            .map(|bytes| {
                if !bytes
                    .par_iter_combinations::<(EntityId, T), _, N>(|x| (f)(x.map(|(id, item)| unsafe { transmute((*id, item)) }))) {
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
            self
                .components
                .write()
                .insert(type_id, vec);
        }
    }
}