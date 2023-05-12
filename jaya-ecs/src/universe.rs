use std::{any::TypeId, hint::unreachable_unchecked, sync::atomic::{AtomicPtr, Ordering, AtomicU64}, marker::PhantomPinned, pin::Pin, ops::Deref};

use crossbeam::{queue::{SegQueue, ArrayQueue}};
use dashmap::DashMap;
use fxhash::{FxHashMap, FxBuildHasher};
use parking_lot::RwLock;
use rayon::{prelude::{ParallelIterator, FromParallelIterator}, iter::empty};

use crate::{
    collections::AnyVec,
    component::{Component},
    entity::{EntityId},
    extract::{
        ComponentModifier, ComponentModifierStager, MultiComponentModifier,
        MultiComponentModifierStager,
    },
};

struct Unmovable<T>(Pin<Box<(T, PhantomPinned)>>);

impl<T> Deref for Unmovable<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.0.0
    }
}

impl<T> From<T> for Unmovable<T> {
    fn from(value: T) -> Self {
        Self(Box::pin((value, PhantomPinned)))
    }
}

#[repr(C)]
struct PackedComponent<C> {
    ptr: Box<AtomicPtr<u8>>,
    id: EntityId,
    component: C
}

struct RemoveComponentData {
    component_raw_ptr: *const AtomicPtr<u8>,
    component_vec_ptr: *const AnyVec,
}

/// A concurrent map of entities and pointers to their `Component`s
struct EntityMap {
    map: DashMap<EntityId, Vec<RemoveComponentData>, FxBuildHasher>,
    empty_vecs: ArrayQueue<Vec<RemoveComponentData>>,
    id_counter: AtomicU64
}


impl Default for EntityMap {
    fn default() -> Self {
        Self {
            map: Default::default(),
            empty_vecs: ArrayQueue::new(256), 
            id_counter: Default::default()
        }
    }
}

impl EntityMap {
    fn add_entity<const N: usize>(&self, component_ptrs: [RemoveComponentData; N]) -> EntityId {
        let mut vec = self.empty_vecs.pop().unwrap_or_else(|| Vec::with_capacity(N));
        vec.extend(component_ptrs);
        // In the situation that the id counter overflows,
        // all that would happen is the programmer will lose the ability to
        // delete overwritten entities. However, components of those entities
        // will still be processed
        //
        // To overflow a u64, the programmer must be adding 2.135×10^13
        // entities, 240 times a second, for one hour. So I figured that
        // checking for overflows is unnecessary
        let id = EntityId(self.id_counter.fetch_add(1, Ordering::Relaxed));
        self.map.insert(id, vec);
        id
    }

    /// # Safety
    /// Behaviour is undefined if any of the following isn't true
    /// 1. There must not be any thread that is adding components
    unsafe fn remove_entity(&mut self, id: EntityId) -> bool {
        if let Some((_, mut vec)) = self.map.remove(&id) {
            for remove_data in vec.drain(..) {
                let ptr = (*remove_data.component_raw_ptr).load(Ordering::Relaxed);

                if (*remove_data.component_vec_ptr).swap_remove_by_ptr(ptr) {
                    let c_ptr: *mut Box<AtomicPtr<u8>> = ptr.cast();
                    *(*c_ptr).get_mut() = ptr;
                }
            }
            let _ = self.empty_vecs.push(vec);
            true
        } else {
            false
        }
    }
}

unsafe impl Send for RemoveComponentData { }
unsafe impl Sync for RemoveComponentData { }

/// A Universe is a container for components, and an optional extension object
/// 
/// A Universe allows Systems to be ran, and modifications to components to be done in parallel
#[derive(Default)]
pub struct Universe<T=()> {
    components: FxHashMap<TypeId, Unmovable<AnyVec>>,
    mod_queue: ComponentModifierStager,
    multi_mod_queue: MultiComponentModifierStager,
    new_components_map: RwLock<FxHashMap<TypeId, Unmovable<AnyVec>>>,
    remove_queue: SegQueue<EntityId>,
    entity_map: EntityMap,
    extension: T
}

impl<S> Universe<S> {
    /// Borrows the extension
    #[inline(always)]
    pub fn get_extension(&self) -> &S {
        &self.extension
    }

    /// Adds an entity with the given components
    /// 
    /// The given components should be in a tuple (even if it is just one component).
    /// An EntityId is assigned to it and returned
    #[inline(always)]
    pub fn add_entity<C: ComponentsContainer>(&self, components: C) -> EntityId {
        components.add_to_universe(self)
    }

    /// Runs the given function on all components of a given type `T`
    pub fn iter_components<'a, T, F>(&'a self, f: F)
    where
        T: Component + 'static,
        F: Fn(EntityId, &'a T) + Sync,
    {
        self.components
            .get(&TypeId::of::<T>())
            .map(|bytes| unsafe {
                bytes
                    .par_iter::<PackedComponent<T>>()
                    .unwrap_unchecked()
                    .for_each(|x| (f)(x.id, &x.component));
            });
    }

    /// Runs the given function on all combinations of components of a given type `T`.
    /// 
    /// Combinations are like permutations, except that the order of the components does not matter
    pub fn iter_component_combinations<'a, T, F, const N: usize>(&'a self, f: F)
    where
        T: Component + 'static,
        F: Fn([(EntityId, &'a T); N]) + Sync,
    {
        self.components.get(&TypeId::of::<T>()).map(|bytes| {
            if !bytes.par_iter_combinations::<PackedComponent<T>, _, N>(|x| {
                (f)(x.map(|x| (x.id, &x.component)))
            }) {
                unsafe { unreachable_unchecked() }
            }
        });
    }

    /// Runs the given function on all components of a given type `T`, and collects the result
    pub fn iter_components_collect<'a, T, F, V, C>(&'a self, f: F) -> C
    where
        V: Send,
        T: Component + 'static,
        F: Fn(EntityId, &'a T) -> V + Sync,
        C: FromParallelIterator<V>
    {
        self.components
            .get(&TypeId::of::<T>())
            .map(|bytes| unsafe {
                bytes
                    .par_iter::<PackedComponent<T>>()
                    .unwrap_unchecked()
                    .map(|x| (f)(x.id, &x.component))
                    .collect()
            })
            .unwrap_or_else(|| empty().collect())
    }

    /// Queues an entity for removal
    /// 
    /// Entities will *never* be removed instantly
    pub fn queue_remove_entity(&self, id: EntityId) {
        self.remove_queue.push(id);
    }

    /// Queues a component modification
    /// 
    /// Modifications will *never* be applied instantly
    pub(crate) fn queue_component_modifier(&self, modifier: ComponentModifier) {
        self.mod_queue.queue_modifier(modifier);
    }

    /// Queues a multi component modification
    /// 
    /// Modifications will *never* be applied instantly
    pub(crate) fn queue_multi_component_modifier(&self, modifier: MultiComponentModifier) {
        self.multi_mod_queue.queue_modifier(modifier);
    }

    /// Clears out remove and modifier queues and applies the changes
    pub fn process_queues(&mut self) {
        unsafe {
            self.multi_mod_queue.execute();
            self.mod_queue.execute();
        }
        self.components.extend(self.new_components_map.get_mut().drain());
        while let Some(id) = self.remove_queue.pop() {
            unsafe {
                self.entity_map.remove_entity(id);
            }
        }
    }
}

/// You should not need to implement this yourself
pub trait ComponentsContainer {
    fn add_to_universe<S>(self, universe: &Universe<S>) -> EntityId;
}

impl<C1: Component + 'static> ComponentsContainer for (C1,) {
    fn add_to_universe<S>(self, universe: &Universe<S>) -> EntityId {
        let c1 = self.0;
        let c1_ptr = Box::new(AtomicPtr::<u8>::default());
        let c1_ptr_ptr: *const _ = c1_ptr.deref();
        let type_id = TypeId::of::<C1>();
        let id;

        macro_rules! data {
            ($vec: expr) => {
                [RemoveComponentData {
                    component_raw_ptr: (c1_ptr.deref() as *const AtomicPtr<u8>).into(),
                    component_vec_ptr: $vec.deref(),
                }]
            };
        }
        macro_rules! packed {
            ($id: expr) => {
                PackedComponent {
                    ptr: c1_ptr,
                    id: $id,
                    component: c1
                }
            };
        }

        if let Some(vec) = universe.components.get(&type_id) {
            id = universe.entity_map.add_entity(data!(vec));

            unsafe {
                let insert_ptr = vec.push(packed!(id)).unwrap_unchecked();
                (*c1_ptr_ptr).store(insert_ptr, Ordering::Relaxed);
            }
        } else {
            {
                let reader = universe.new_components_map.read();
                if let Some(vec) = reader.get(&type_id) {
                    id = universe.entity_map.add_entity(data!(vec));
                    unsafe {
                        let insert_ptr = vec.push(packed!(id)).unwrap_unchecked();
                        (*c1_ptr_ptr).store(insert_ptr, Ordering::Relaxed);
                    }
                    return id
                }
            }
            let vec = Unmovable::from(AnyVec::new::<PackedComponent<C1>>());
            id = universe.entity_map.add_entity(data!(vec));
            unsafe {
                let insert_ptr = vec.push(packed!(id)).unwrap_unchecked();
                (*c1_ptr_ptr).store(insert_ptr, Ordering::Relaxed);
            }
            universe.new_components_map.write().insert(type_id, vec);
        }
        id
    }
}


/// Trait for structs that contains a sub state
/// 
/// You can think of this as another type of `AsRef`
pub trait AsSubState<S> {
    /// Extracts a sub state from `self`
    fn as_sub_state(&self) -> &S;
}
