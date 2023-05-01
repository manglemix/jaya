#![feature(associated_type_defaults)]
#![feature(never_type)]
#![feature(iter_collect_into)]
#![feature(unboxed_closures)]
#![feature(fn_traits)]

pub mod system;
pub mod extract;
pub mod component;
pub mod entity;
pub mod container;
pub mod universe;

pub use rayon;

// #[cfg(test)]
// mod tests {
//     use std::time::{UNIX_EPOCH};

//     use fxhash::FxHashMap;
//     use rayon::{collections::hash_map::Iter, prelude::ParallelIterator};

//     use concurrent_queue::ConcurrentQueue;
//     use rayon::{prelude::IntoParallelRefIterator, iter::Map};

//     use crate::{component::{ComponentContainer, Component, ComponentModifierContainer}, entity::{EntityId, AddEntity}, extract::{ComponentModifier, Query, ComponentModifierStager, MultiComponentModifierStager, MultiQuery}, system::System};

//     #[derive(Default, Debug)]
//     struct SimpleComponent {
//         value: usize
//     }

//     impl Component for SimpleComponent { }

//     struct Entities {
//         entities: FxHashMap<EntityId, SimpleComponent>,
//         add_queue: ConcurrentQueue<(EntityId, SimpleComponent)>,
//         remove_queue: ConcurrentQueue<EntityId>,
//         mod_queue: ComponentModifierStager,
//         multi_mod_queue: MultiComponentModifierStager
//     }

//     impl<'a> ComponentContainer<'a, SimpleComponent> for Entities {
//         type Iterator = Map<Iter<'a, EntityId, SimpleComponent>, fn((&'a EntityId, &'a SimpleComponent)) -> (EntityId, &'a SimpleComponent)>;

//         fn queue_add_component(&self, id: crate::entity::EntityId, component: SimpleComponent) {
//             assert!(self.add_queue.push((id, component)).is_ok());
//         }

//         fn queue_remove_component(&self, id: crate::entity::EntityId) {
//             assert!(self.remove_queue.push(id).is_ok());
//         }

//         fn iter_components(&'a self) -> Self::Iterator {
//             self.entities.par_iter().map(|x| (*x.0, x.1))
//         }
//     }

//     impl ComponentModifierContainer for Entities {
//         fn queue_component_modifier(&self, modifier: ComponentModifier) {
//             self.mod_queue.queue_modifier(modifier);
//         }

//         fn queue_multi_component_modifier(&self, modifier: crate::extract::MultiComponentModifier) {
//             self.multi_mod_queue.queue_modifier(modifier);
//         }
//     }

//     impl Entities {
//         pub fn process_queues(&mut self) {
//             unsafe {
//                 self.multi_mod_queue.execute();
//                 self.mod_queue.execute();
//             }
//             for (id, component) in self.add_queue.try_iter() {
//                 self.entities.insert(id, component);
//             }
//             for id in self.remove_queue.try_iter() {
//                 self.entities.remove(&id);
//             }
//         }
//     }

//     fn incrementor(query: Query<SimpleComponent, Entities>) {
//         query.queue_mut(|x| {
//             x.value += 1;
//         });
//     }

//     fn summator(query1: Query<SimpleComponent, Entities>, query2: Query<SimpleComponent, Entities>) {
//         (query1, query2)
//             .queue_mut(|(query1, query2)| {
//                 let sum = query1.value.checked_add(query2.value).unwrap_or_default();
//                 query1.value = sum;
//                 query2.value = sum;
//             })
//     }

//     fn maker(universe: &Entities) {
//         universe.queue_add_entity(EntityId::from(UNIX_EPOCH.elapsed().unwrap().as_micros() as usize), (SimpleComponent::default(),));
//     }

//     fn nothing() {

//     }

//     #[test]
//     fn test01() {
//         let mut entities = Entities {
//             entities: Default::default(),
//             add_queue: ConcurrentQueue::unbounded(),
//             remove_queue: ConcurrentQueue::unbounded(),
//             mod_queue: ComponentModifierStager::new(),
//             multi_mod_queue: Default::default()
//         };

//         for _i in 0..100 {
//             incrementor.run_once(&entities);
//             summator.run_once(&entities);
//             maker.run_once(&entities);
//             nothing.run_once(&entities);

//             entities.process_queues();
//         }

//         eprintln!("{:?}", entities.entities);
//     }
// }