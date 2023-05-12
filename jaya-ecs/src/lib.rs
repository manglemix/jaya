//! A simple implementation of an Entity Component System
//!
//! Emphasis is placed on ergonomics and massive parallelism.
#![feature(maybe_uninit_uninit_array)]
#![feature(sync_unsafe_cell)]

pub mod collections;
pub mod component;
pub mod entity;
pub mod extract;
pub mod system;
pub mod universe;

pub use rayon;
