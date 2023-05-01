use std::{ops::Deref, ptr::NonNull, marker::PhantomData, sync::Arc, iter::repeat};

mod subextract;

use crate::{entity::{EntityId}, component::{Component, ComponentContainer, ComponentModifierContainer}};

use rayon::{iter::{Map, Once, once}, prelude::{ParallelIterator, ParallelBridge}};
pub use subextract::{MultiComponentModifier, MultiQuery, MultiComponentModifierStager, ComponentModifierStager, ComponentModifier};


pub trait FromUniverse<'a, S>: Send + Clone {
    type Iterator: ParallelIterator<Item = Self> = Once<Self>;
    
    fn get_choices(universe: &'a S) -> Self::Iterator;
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
    T: Sync,
    S: AsRef<T>
{
    fn get_choices(universe: &'a S) -> Self::Iterator {
        once(State(universe.as_ref()))
    }
}

pub struct Query<'a, T>
where
    T: Component + 'a,
    // S: ComponentContainer<'a, T>
{
    id: EntityId,
    component: &'a T,
    universe: &'a dyn ComponentContainer<'a, T>
}


impl<'a, T, S> PartialEq for Query<'a, T, S>
where
    T: Component + 'a,
    S: ComponentContainer<'a, T>
{
    fn eq(&self, other: &Self) -> bool {
        std::ptr::eq(self.component, other.component)
    }
}


impl<'a, T, S> Eq for Query<'a, T, S>
where
    T: Component + 'a,
    S: ComponentContainer<'a, T>
{ }

impl<'a, T, S> Clone for Query<'a, T, S>
where
    T: Component + 'a,
    S: ComponentContainer<'a, T>
{
    fn clone(&self) -> Self {
        *self
    }
}

impl<'a, T, S> Copy for Query<'a, T, S>
where
    T: Component + 'a,
    S: ComponentContainer<'a, T>
{ }

impl<'a, T, S> Deref for Query<'a, T, S>
where
    T: Component + 'a,
    S: ComponentContainer<'a, T>
{
    type Target = T;

    fn deref(&self) -> &Self::Target {
        self.component
    }
}

pub struct QueryMapFn<'a, T, S> {
    universe: &'a S,
    _phantom: PhantomData<T>
}


impl<'a, T, S> FnOnce<((EntityId, &'a T),)> for QueryMapFn<'a, T, S>
where
    T: Component,
    S: ComponentContainer<'a, T>
{
    type Output = Query<'a, T, S>;

    extern "rust-call" fn call_once(self, args: ((EntityId, &'a T),)) -> Self::Output {
        Query {
            id: args.0.0,
            component: args.0.1,
            universe: self.universe
        }
    }

}

impl<'a, T, S> FnMut<((EntityId, &'a T),)> for QueryMapFn<'a, T, S>
where
    T: Component,
    S: ComponentContainer<'a, T>
{
    extern "rust-call" fn call_mut(&mut self, args: ((EntityId, &'a T),)) -> Self::Output {
        Query {
            id: args.0.0,
            component: args.0.1,
            universe: self.universe
        }
    }
}


impl<'a, T, S> Fn<((EntityId, &'a T),)> for QueryMapFn<'a, T, S>
where
    T: Component,
    S: ComponentContainer<'a, T>
{
    extern "rust-call" fn call(&self, args: ((EntityId, &'a T),)) -> Self::Output {
        Query {
            id: args.0.0,
            component: args.0.1,
            universe: self.universe
        }
    }
}

impl<'a, T, S> FromUniverse<'a, S> for Query<'a, T, S>
where
    T: Component + 'a,
    S: ComponentContainer<'a, T> + Sync
{
    type Iterator = Map<S::Iterator, QueryMapFn<'a, T, S>>;

    fn get_choices(universe: &'a S) -> Self::Iterator {
        universe.iter_components().map(QueryMapFn { universe, _phantom: Default::default() })
    }
}

impl<'a, T, S> Query<'a, T, S>
where
    T: Component + 'a,
    S: ComponentContainer<'a, T> + ComponentModifierContainer,
{
    pub fn get_id(&self) -> EntityId {
        self.id
    }
    pub fn queue_mut(&self, f: impl FnOnce(&mut T) + 'static) {
        let _ = self
            .universe
            .queue_component_modifier(
                ComponentModifier {
                    ptr: NonNull::from(self.component).cast(),
                    f: Box::new(|x| unsafe { (f)(x.cast::<T>().as_mut()) })
                }
            );
    }
}

impl<'a, S: Sync> FromUniverse<'a, S> for &'a S {
    fn get_choices(universe: &'a S) -> Self::Iterator {
        once(universe)
    }
}

// pub struct CombinationQuery<'a, T, S, const N: usize = 2> {
//     components: [&'a T; N],
//     universe: &'a S
// }


// impl<'a, T, S> Deref for CombinationQuery<'a, T, S> {
//     type Target = T;

//     fn deref(&self) -> &Self::Target {
//         self.components
//     }
// }


// impl<'a, T, S> Clone for CombinationQuery<'a, T, S> {
//     fn clone(&self) -> Self {
//         *self
//     }
// }

// impl<'a, T, S> Copy for CombinationQuery<'a, T, S> { }

// impl<'a, S, C1, C2> FromUniverse<'a, S> for CombinationQuery<'a, (C1, C2), S>
// where
//     C1: Component + 'a,
//     C2: Component + 'a,
//     S: ComponentContainer<'a, C1> + ComponentContainer<'a, C2> + Sync
// {
//     type Iterator = Once<Self>;

//     fn get_choices(universe: &'a S) -> Self::Iterator {
//         let u_vec: Vec<_> = ComponentContainer::<C1>::iter_components(universe).collect();
//         let v_vec: Vec<_> = ComponentContainer::<C2>::iter_components(universe).collect();

//         BiCombinator {
//             u_vec,
//             v_vec,
//             u_i: 0,
//             v_i: 0
//         }.par_bridge()
//     }
// }