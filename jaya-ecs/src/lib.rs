// #![feature(associated_type_defaults)]
// #![feature(never_type)]
// #![feature(iter_collect_into)]
// #![feature(unboxed_closures)]
// #![feature(fn_traits)]
// #![feature(once_cell)]
// #![feature(generic_const_exprs)]
#![feature(maybe_uninit_uninit_array)]

pub mod system;
pub mod extract;
pub mod component;
pub mod entity;
pub mod container;
pub mod universe;
pub mod collections;

pub use rayon;

// #[cfg(test)]
// mod tests {
//     use std::{collections::HashSet, hash::Hasher};

//     use hashers::jenkins::spooky_hash::SpookyHasher;
//     use rayon::prelude::{IntoParallelIterator, ParallelIterator, IndexedParallelIterator};

//     #[test]
//     fn hash_test() {
//         let mut seen = HashSet::new();

//         (0..usize::MAX)
//             .into_par_iter()
//             .fold_chunks(u32::MAX, todo!(), |a, b| {

//             })
//             .map(|x| {

//             });

//         for i in 0..usize::MAX {
//             let mut hasher = SpookyHasher::default();
//             // hasher.write_u8(i as u8);
//             hasher.write_usize(i);
//             let hash = hasher.finish();
//             assert!(seen.insert(hash), "usize: {} hash: {} set_len: {}", i, hash, seen.len())
//         }
//     }
// }