//! Structs which implement `FromUniverse` are extractors, and can be used as parameters on functions to make them `System`s
use std::{ops::Deref, ptr::NonNull};

mod subextract;

use crate::{
    component::Component,
    entity::EntityId,
    universe::{AsSubState, Universe},
};

pub use subextract::{ComponentModifierStager, MultiComponentModifierStager, MultiQuery};

pub(crate) use self::subextract::{ComponentModifier, MultiComponentModifier};

/// A trait for structs that can be produced from a `Universe`
pub trait FromUniverse<'a, S>: Send + Clone + 'a {
    /// Calls the given function with as many variations of `self` as possible
    fn iter_choices<F>(universe: &'a Universe<S>, f: F)
    where
        F: Fn(Self) + Sync;
}

/// Extractor for a state from a `Universe`
pub struct State<'a, T: Sync>(pub &'a T);

impl<'a, T: Sync> Clone for State<'a, T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<'a, T: Sync> Copy for State<'a, T> {}

impl<'a, T, S> FromUniverse<'a, S> for State<'a, T>
where
    T: Sync + 'a,
    S: AsSubState<T>,
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

/// A `Query` is an immutable reference to a `Component`
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
    S: Sync,
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
    /// Gets the `EntityId` of the Entity that the `Component` is for
    pub fn get_id(&self) -> EntityId {
        self.id
    }
    /// Queues a modification for this `Component`
    ///
    /// This is the fastest way to modify a `Component`, but it only allows
    /// for one mutable reference to one `Component` at a time. If you need
    /// to have multiple mutable references, combine the queries for the
    /// components you wish to modify into a tuple and use that instead.
    ///
    /// Do keep in mind that this approach is *far* slower, so do not hesitate
    /// to break up a modification into multiple individual modifiers.
    ///
    /// Modifications will *never* be ran immediately.
    pub fn queue_mut(&self, f: impl FnOnce(&mut T) + 'static) {
        self.universe.queue_component_modifier(ComponentModifier {
            ptr: NonNull::from(self.component).cast(),
            f: Box::new(|x| unsafe { (f)(x.cast::<T>().as_mut()) }),
        });
    }
}

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
    S: Sync,
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
