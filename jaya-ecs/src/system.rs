use rayon::join;

use crate::{extract::FromUniverse, universe::Universe};

pub trait System<'a, T, S> {
    fn run_once(&self, universe: &'a Universe<S>);
}

impl<'a, T: Fn(), S> System<'a, (), S> for T {
    fn run_once(&self, _universe: &'a Universe<S>) {
        (self)()
    }
}

impl<'a, A1, T, S> System<'a, (A1,), S> for T
where
    T: Fn(A1) + Sync,
    A1: FromUniverse<'a, S>,
{
    fn run_once(&self, universe: &'a Universe<S>) {
        A1::iter_choices(universe, self);
    }
}

impl<'a, A1, A2, T, S> System<'a, (A1, A2), S> for T
where
    T: Fn(A1, A2) + Sync,
    A1: FromUniverse<'a, S> + Sync,
    A2: FromUniverse<'a, S>,
    S: Sync
{
    fn run_once(&self, universe: &'a Universe<S>) {
        A1::iter_choices(universe, |c1| {
            A2::iter_choices(universe, |c2| (self)(c1.clone(), c2))
        });
    }
}

impl<'a, S, A1, A2, C1, C2> System<'a, (A1, A2), S> for (C1, C2)
where
    C1: System<'a, A1, S> + Sync,
    C2: System<'a, A2, S> + Sync,
    S: Sync
{
    fn run_once(&self, universe: &'a Universe<S>) {
        join(
            || {
                self.0.run_once(universe);
            },
            || {
                self.1.run_once(universe);
            },
        );
    }
}

impl<'a, S, A1, A2, A3, C1, C2, C3> System<'a, (A1, A2, A3), S> for (C1, C2, C3)
where
    C1: System<'a, A1, S> + Sync,
    C2: System<'a, A2, S> + Sync,
    C3: System<'a, A3, S> + Sync,
    S: Sync
{
    fn run_once(&self, universe: &'a Universe<S>) {
        join(
            || {
                self.0.run_once(universe);
            },
            || {
                join(
                    || {
                        self.1.run_once(universe);
                    },
                    || {
                        self.2.run_once(universe);
                    }
                )
            },
        );
    }
}
