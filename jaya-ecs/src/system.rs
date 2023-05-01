use rayon::prelude::ParallelIterator;

use crate::{extract::FromUniverse, universe::Universe};


pub trait System<'a, T, S: Universe> {
    fn run_once(&self, universe: &'a S);
}


impl<'a, T: Fn(), S: Universe> System<'a, (), S> for T {
    fn run_once(&self, _universe: &'a S) {
        (self)()
    }
}


impl<'a, A1, T, S: Universe> System<'a, (A1,), S> for T
where
    T: Fn(A1) + Sync,
    A1: FromUniverse<'a, S>
{
    fn run_once(&self, universe: &'a S) {
        A1::get_choices(universe)
            .for_each(self);
    }
}


impl<'a, A1, A2, T, S> System<'a, (A1, A2), S> for T
where
    T: Fn(A1, A2) + Sync,
    A1: FromUniverse<'a, S> + Sync,
    A2: FromUniverse<'a, S>,
    S: Universe
{
    fn run_once(&self, universe: &'a S) {
        A1::get_choices(universe)
            .for_each(|choice1| {
                A2::get_choices(universe)
                    .for_each(|choice2| {
                        (self)(choice1.clone(), choice2)
                    });
            });
    }
}