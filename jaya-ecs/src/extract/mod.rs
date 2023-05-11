use std::{ops::Deref, ptr::NonNull};

mod subextract;

use crate::{
    component::{Component},
    entity::EntityId,
    universe::{Universe, AsSubState},
};

pub use subextract::{
    ComponentModifier, ComponentModifierStager, MultiComponentModifier,
    MultiComponentModifierStager, MultiQuery,
};

pub trait FromUniverse<'a, S>: Send + Clone + 'a {
    fn iter_choices<F>(universe: &'a Universe<S>, f: F)
    where
        F: Fn(Self) + Sync;
}

pub struct State<'a, T: Sync>(pub &'a T);

impl<'a, T: Sync> Clone for State<'a, T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<'a, T: Sync> Copy for State<'a, T> { }

impl<'a, T, S> FromUniverse<'a, S> for State<'a, T>
where
    T: Sync + 'a,
    S: AsSubState<T>
{
    fn iter_choices<F>(universe: &'a Universe<S>, f: F)
    where
        F: Fn(Self) + Sync,
    {
        (f)(State(universe.get_extension().as_sub_state()))
    }
}

trait UniverseRef: Sync {
    fn queue_component_modifier(&self, modifier: ComponentModifier);
    fn queue_multi_component_modifier(&self, modifier: MultiComponentModifier);
}


impl<S: Sync> UniverseRef for Universe<S> {
    #[inline(always)]
    fn queue_component_modifier(&self, modifier: ComponentModifier) {
        Universe::queue_component_modifier(self, modifier);
    }

    #[inline(always)]
    fn queue_multi_component_modifier(&self, modifier: MultiComponentModifier) {
        Universe::queue_multi_component_modifier(self, modifier);
    }
}


pub struct Query<'a, T>
where
    T: Component,
{
    id: EntityId,
    component: &'a T,
    universe: &'a (dyn UniverseRef),
}

impl<'a, T> PartialEq for Query<'a, T>
where
    T: Component,
{
    fn eq(&self, other: &Self) -> bool {
        std::ptr::eq(self.component, other.component)
    }
}

impl<'a, T> Eq for Query<'a, T> where T: Component {}

impl<'a, T> Clone for Query<'a, T>
where
    T: Component,
{
    fn clone(&self) -> Self {
        *self
    }
}

impl<'a, T> Copy for Query<'a, T> where T: Component {}

impl<'a, T> Deref for Query<'a, T>
where
    T: Component,
{
    type Target = T;

    fn deref(&self) -> &Self::Target {
        self.component
    }
}

impl<'a, T, S> FromUniverse<'a, S> for Query<'a, T>
where
    T: Component + 'static,
    S: Sync
{
    fn iter_choices<F>(universe: &'a Universe<S>, f: F)
    where
        F: Fn(Self) + Sync,
    {
        universe.iter_components(|id, component| {
            (f)(Query {
                id,
                component,
                universe,
            });
        });
    }
}

impl<'a, T> Query<'a, T>
where
    T: Component,
{
    pub fn get_id(&self) -> EntityId {
        self.id
    }
    pub fn queue_mut(&self, f: impl FnOnce(&mut T) + 'static) {
        self.universe.queue_component_modifier(ComponentModifier {
            ptr: NonNull::from(self.component).cast(),
            f: Box::new(|x| unsafe { (f)(x.cast::<T>().as_mut()) }),
        });
    }
}

// impl<'a, S: Sync> FromUniverse<'a, S> for &'a S {
//     fn iter_choices<F>(universe: &'a Universe<S>, f: F)
//     where
//         F: Fn(Self) + Sync,
//     {
//         (f)(&universe.get_extension())
//     }
// }

impl<'a, S: Sync> FromUniverse<'a, S> for &'a Universe<S> {
    fn iter_choices<F>(universe: &'a Universe<S>, f: F)
    where
        F: Fn(Self) + Sync,
    {
        (f)(universe)
    }
}

impl<'a, T, S, const N: usize> FromUniverse<'a, S> for [Query<'a, T>; N]
where
    T: Component + 'static,
    S: Sync
{
    fn iter_choices<F>(universe: &'a Universe<S>, f: F)
    where
        F: Fn(Self) + Sync,
    {
        universe.iter_component_combinations(|arr| {
            (f)(arr.map(|(id, component)| Query {
                id,
                component,
                universe,
            }))
        });
    }
}
