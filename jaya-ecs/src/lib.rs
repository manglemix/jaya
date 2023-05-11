// #![feature(associated_type_defaults)]
// #![feature(never_type)]
// #![feature(iter_collect_into)]
// #![feature(unboxed_closures)]
// #![feature(fn_traits)]
// #![feature(once_cell)]
// #![feature(generic_const_exprs)]
#![feature(maybe_uninit_uninit_array)]
#![feature(sync_unsafe_cell)]

pub mod collections;
pub mod component;
pub mod entity;
pub mod extract;
pub mod system;
pub mod universe;

pub use rayon;
