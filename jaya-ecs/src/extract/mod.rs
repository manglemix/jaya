use std::{ops::Deref, ptr::NonNull};

mod subextract;

use crate::{entity::{EntityId}, component::{Component, IntoComponentIterator}, universe::Universe};

pub use subextract::{MultiComponentModifier, MultiQuery, MultiComponentModifierStager, ComponentModifierStager, ComponentModifier};


pub trait FromUniverse<'a, S>: Send + Clone + 'a {
    fn iter_choices<F>(universe: &'a S, f: F) where F: Fn(Self) + Sync;
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
    S: AsRef<T>
{
    fn iter_choices<F>(universe: &'a S, f: F) where F: Fn(Self) + Sync {
        (f)(State(universe.as_ref()))
    }
}

pub struct Query<'a, T>
where
    T: Component
{
    id: EntityId,
    component: &'a T,
    universe: &'a (dyn Universe)
}


impl<'a, T> PartialEq for Query<'a, T>
where
    T: Component
{
    fn eq(&self, other: &Self) -> bool {
        std::ptr::eq(self.component, other.component)
    }
}


impl<'a, T> Eq for Query<'a, T>
where
    T: Component
{ }

impl<'a, T> Clone for Query<'a, T>
where
    T: Component
{
    fn clone(&self) -> Self {
        *self
    }
}

impl<'a, T> Copy for Query<'a, T>
where
    T: Component
{ }

impl<'a, T> Deref for Query<'a, T>
where
    T: Component
{
    type Target = T;

    fn deref(&self) -> &Self::Target {
        self.component
    }
}


impl<'a, T, S> FromUniverse<'a, S> for Query<'a, T>
where
    T: Component + 'a,
    S: IntoComponentIterator<T> + Universe
{
    fn iter_choices<F>(universe: &'a S, f: F) where F: Fn(Self) + Sync {
        universe.iter_components(|id, component| {
            (f)(Query {
                id,
                component,
                universe,
            })
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
        self
            .universe
            .queue_component_modifier(
                ComponentModifier {
                    ptr: NonNull::from(self.component).cast(),
                    f: Box::new(|x| unsafe { (f)(x.cast::<T>().as_mut()) })
                }
            );
    }
}

impl<'a, S: Universe> FromUniverse<'a, S> for &'a S {
    fn iter_choices<F>(universe: &'a S, f: F) where F: Fn(Self) + Sync {
        (f)(universe)
    }
}


pub type AnyUniverse<'a> = &'a dyn Universe;


impl<'a, S: Universe> FromUniverse<'a, S> for AnyUniverse<'a> {
    fn iter_choices<F>(universe: &'a S, f: F) where F: Fn(Self) + Sync {
        (f)(universe)
    }
}

impl<'a, T, S, const N: usize> FromUniverse<'a, S> for [Query<'a, T>; N]
where
    T: Component + 'a,
    S: IntoComponentIterator<T> + Universe
{
    fn iter_choices<F>(universe: &'a S, f: F) where F: Fn(Self) + Sync {
        universe.iter_component_combinations(|arr| {
            (f)(arr.map(|(id, component)| {
                Query {
                    id,
                    component,
                    universe,
                }
            }))
        });
    }
}