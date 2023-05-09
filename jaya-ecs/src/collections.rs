use std::{
    any::TypeId,
    cell::SyncUnsafeCell,
    hint::unreachable_unchecked,
    marker::PhantomData,
    mem::{forget, size_of, MaybeUninit},
    ops::Deref,
    ptr::{copy_nonoverlapping, drop_in_place},
    sync::atomic::{AtomicBool, AtomicUsize, Ordering}, num::NonZeroUsize,
};

use crossbeam::utils::Backoff;
use rayon::{
    prelude::{IndexedParallelIterator, IntoParallelRefIterator, ParallelIterator},
    slice::ParallelSlice,
};

const DEFAULT_BLOCK_SIZE: usize = 32;
const BLOCK_GROWTH_RATE: usize = 2;

pub struct AnyVec {
    bytes: SyncUnsafeCell<Vec<Box<[u8]>>>,
    maybe_init_len: AtomicUsize,
    init_len: AtomicUsize,
    resizing: AtomicBool,
    type_id: TypeId,
    dropper: fn(&mut Self)
}

impl AnyVec {
    pub fn new<T: 'static>() -> Self {
        Self::with_capacity::<T>(DEFAULT_BLOCK_SIZE.try_into().unwrap())
    }

    pub fn with_capacity<T: 'static>(capacity: NonZeroUsize) -> Self {
        let capacity = capacity.get();
        let block = vec![0u8; capacity * size_of::<T>()];
        Self {
            bytes: SyncUnsafeCell::new(vec![block.into_boxed_slice()]),
            resizing: Default::default(),
            maybe_init_len: Default::default(),
            type_id: TypeId::of::<T>(),
            init_len: Default::default(),
            dropper: |mutref| unsafe {
                debug_assert_eq!(mutref.init_len.get_mut(), mutref.maybe_init_len.get_mut());
                let mut init_len = *mutref.init_len.get_mut();

                for block in mutref.bytes.get_mut() {
                    let ptr: *mut u8 = block.get_unchecked_mut(0);
                    let ptr: *mut T = ptr.cast();

                    for i in 0..(block.len() / size_of::<T>()) {
                        drop_in_place(ptr.offset(i as isize));
                        init_len -= 1;
                        if init_len == 0 {
                            return
                        }
                    }
                }
            },
        }
    }

    pub fn get<T: 'static>(&self, index: usize) -> Option<&T> {
        if TypeId::of::<T>() != self.type_id {
            return None;
        }

        if index >= self.init_len.load(Ordering::Relaxed) {
            return None;
        }

        unsafe {
            let ptr = AnyVec::get_bytes_ptr::<T>(index, &(*self.bytes.get()));
            Some(&(*ptr.cast()))
        }
    }

    #[must_use]
    pub fn push<T: Send + Sync + 'static>(&self, value: T) -> Option<T> {
        if TypeId::of::<T>() != self.type_id {
            return Some(value);
        }

        let t_size = size_of::<T>();
        let t_ptr: *const T = &value;
        let t_ptr: *const u8 = t_ptr.cast();
        let current_len = self.maybe_init_len.fetch_add(1, Ordering::Acquire) + 1;
        let backoff = Backoff::new();

        loop {
            let last_block_len;

            {
                let reader = unsafe { &(*self.bytes.get()) };

                if current_len >= Self::get_capacity::<T>(reader.deref()) {
                    let last_resizing_value = self.resizing.swap(true, Ordering::Acquire);
                    if last_resizing_value {
                        // another thread is already resizing
                        backoff.snooze();
                        continue;
                    }
                    last_block_len =
                        unsafe { reader.last().unwrap_unchecked().len() * BLOCK_GROWTH_RATE };
                } else {
                    unsafe {
                        let start_ptr = Self::get_bytes_ptr::<T>(current_len - 1, &reader);
                        copy_nonoverlapping(t_ptr, start_ptr, t_size);
                    }
                    forget(value);

                    backoff.reset();
                    while self.init_len.load(Ordering::Relaxed) != current_len - 1 {
                        backoff.snooze();
                    }
                    self.init_len.fetch_add(1, Ordering::Relaxed);
                    break;
                }
            }

            let new_block = vec![0u8; last_block_len].into_boxed_slice();

            unsafe {
                (&mut (*self.bytes.get())).push(new_block);
            }

            self.resizing.store(false, Ordering::Release);
        }

        None
    }

    pub fn par_iter<'a, T, F>(&'a self, f: F) -> bool
    where
        T: 'static,
        F: Fn(&T) + Sync,
    {
        if TypeId::of::<T>() != self.type_id {
            return false;
        }
        unsafe {
            (&(*self.bytes.get())).par_iter().for_each(|block| {
                block.par_chunks_exact(size_of::<T>()).for_each(|ptr| {
                    let ptr: *const u8 = ptr.get(0).unwrap();
                    let ptr: *const T = ptr.cast();
                    (f)(ptr.as_ref().unwrap())
                });
            });
        }

        true
    }

    pub fn par_iter_combinations<'a, T, F, const N: usize>(&'a self, f: F) -> bool
    where
        T: Sync + 'static,
        F: Fn([&T; N]) + Sync,
    {
        if N == 0 {
            return true;
        }
        let init_len = self.init_len.load(Ordering::Relaxed);
        let bytes = unsafe { &(*self.bytes.get()) };

        bytes
            .par_iter()
            .enumerate()
            .map(|(i, block)| {
                let start_index =
                    (block.len() as f64 - 0.5f64.powi(i.try_into().unwrap())) as usize;
                block
                    .par_chunks_exact(size_of::<T>())
                    .enumerate()
                    .map(move |(j, ptr)| unsafe {
                        let ptr: *const u8 = ptr.get(0).unwrap();
                        let ptr: *const T = ptr.cast();
                        (start_index + j, ptr.as_ref().unwrap())
                    })
            })
            .flatten()
            .for_each(|(i, x)| {
                assert!(N <= 2);
                for j in i..init_len {
                    let mut arr = MaybeUninit::<&T>::uninit_array::<N>();
                    arr[0].write(x);
                    unsafe {
                        let ptr = AnyVec::get_bytes_ptr::<T>(j, bytes);
                        arr[1].write(&(*ptr.cast()));
                    }
                    (f)(arr.map(|x| unsafe { x.assume_init() }))
                }
            });

        true
    }

    pub fn iter<T: 'static>(&self) -> AnyVecIter<T> {
        // if self.mut_iterating.load(Ordering::Relaxed) {
        //     return None
        // }
        AnyVecIter {
            len: if TypeId::of::<T>() == self.type_id {
                self.init_len.load(Ordering::Relaxed)
            } else {
                0
            },
            current_index: 0,
            vec_ref: unsafe { &(*self.bytes.get()) },
            _phantom: Default::default(),
        }
    }

    // pub fn iter_mut<T: 'static>(&self) -> Option<AnyVecIterMut<T>> {
    //     if self.mut_iterating.swap(true, Ordering::Relaxed) {
    //         return None
    //     }
    //     Some(AnyVecIterMut {
    //                 len: if TypeId::of::<T>() == self.type_id {
    //                     self.init_len.load(Ordering::Relaxed)
    //                 } else {
    //                     0
    //                 },
    //                 current_index: 0,
    //                 vec_ref: self.bytes.read(),
    //                 _phantom: Default::default(),
    //                 mut_iterating: &self.mut_iterating,
    //             })
    // }

    // pub fn exclusive_iter_mut<T: 'static>(&mut self) -> AnyVecIterMut<T> {
    //     debug_assert!(!self.mut_iterating.load(Ordering::Relaxed));
    //     self.mut_iterating.store(true, Ordering::Relaxed);
    //     AnyVecIterMut {
    //         len: if TypeId::of::<T>() == self.type_id {
    //             self.init_len.load(Ordering::Relaxed)
    //         } else {
    //             0
    //         },
    //         current_index: 0,
    //         vec_ref: self.bytes.read(),
    //         _phantom: Default::default(),
    //         mut_iterating: &self.mut_iterating,
    //     }
    // }

    // #[must_use]
    // pub fn drop<T: 'static>(mut self) -> Option<Self> {
    //     if TypeId::of::<T>() != self.type_id {
    //         return Some(self)
    //     }
    //     self
    //         .exclusive_iter_mut::<T>()
    //         .for_each(|x| unsafe {
    //             std::ptr::drop_in_place(x);
    //         });
    //     None
    // }

    fn get_capacity<T>(blocks: &[Box<[u8]>]) -> usize {
        blocks
            .into_iter()
            .map(|block| block.len() / size_of::<T>())
            .sum()
    }

    unsafe fn get_bytes_ptr<T>(mut index: usize, blocks: &[Box<[u8]>]) -> *mut u8 {
        debug_assert!(index < Self::get_capacity::<T>(blocks));
        let t_size = size_of::<T>();
        index = index.checked_mul(t_size).unwrap();
        for block in blocks {
            debug_assert!(block.len() % t_size == 0);
            if let Some(x) = block.get(index) {
                let x: *const _ = x;
                return x.cast_mut();
            }
            index -= block.len();
        }
        unreachable_unchecked()
    }
}

pub struct AnyVecIter<'a, T> {
    vec_ref: &'a [Box<[u8]>],
    len: usize,
    current_index: usize,
    _phantom: PhantomData<T>,
}

impl<'a, T: 'a> Iterator for AnyVecIter<'a, T> {
    type Item = &'a T;

    fn next(&mut self) -> Option<Self::Item> {
        if self.current_index >= self.len {
            return None;
        }

        unsafe {
            let ptr = AnyVec::get_bytes_ptr::<T>(self.current_index, &self.vec_ref);
            self.current_index += 1;
            Some(&(*ptr.cast()))
        }
    }
}

impl Drop for AnyVec {
    fn drop(&mut self) {
        (self.dropper)(self);
    }
}

// pub struct AnyVecIterMut<'a, T> {
//     vec_ref: &'a [Box<[u8]>],
//     len: usize,
//     current_index: usize,
//     mut_iterating: &'a AtomicBool,
//     _phantom: PhantomData<T>
// }

// impl<'a, T: 'a> Iterator for AnyVecIterMut<'a, T> {
//     type Item = &'a mut T;

//     fn next(&mut self) -> Option<Self::Item> {
//         if self.current_index >= self.len {
//             return None
//         }

//         unsafe {
//             let ptr = AnyVec::get_bytes_ptr::<T>(self.current_index, &self.vec_ref);
//             self.current_index += 1;
//             Some(&mut (*ptr.cast()))
//         }
//     }
// }

// impl<'a, T> Drop for AnyVecIterMut<'a, T> {
//     fn drop(&mut self) {
//         self.mut_iterating.store(false, Ordering::Relaxed);
//     }
// }

#[cfg(test)]
mod tests {
    use std::{collections::HashSet, ops::Deref, sync::Arc};

    use itertools::Itertools;
    use rayon::prelude::{IntoParallelIterator, ParallelIterator};

    use super::AnyVec;

    #[test]
    fn test01() {
        let vec = AnyVec::new::<usize>();

        (0..500usize).into_iter().for_each(|n| {
            assert_eq!(vec.push(n), None);
        });

        assert_eq!(vec.push(13u8), Some(13u8));
        assert_eq!(
            vec.iter::<usize>().cloned().collect_vec(),
            (0..500).into_iter().collect_vec()
        );
        assert_eq!(vec.iter::<i8>().next(), None);
    }
    #[test]
    fn par_test01() {
        let vec = AnyVec::new::<usize>();

        (0..500usize).into_par_iter().for_each(|n| {
            assert_eq!(vec.push(n), None);
        });

        assert_eq!(vec.push(13u8), Some(13u8));
        assert_eq!(
            vec.iter::<usize>()
                .cloned()
                .collect::<HashSet<usize>>(),
            (0..500).into_iter().collect::<HashSet<usize>>()
        );
        assert_eq!(vec.iter::<i8>().next(), None);
    }
    #[test]
    fn arc_test01() {
        let vec = AnyVec::new::<Arc<(u8, u64, i128)>>();
        let arc = Arc::new((0u8, 2u64, 56i128));
        let weak = Arc::downgrade(&arc);
        assert_eq!(vec.push(arc), None);

        let arc = weak.upgrade().unwrap();
        assert_eq!(arc.deref(), &(0, 2, 56));
        drop(arc);
        assert_eq!(weak.strong_count(), 1);
        drop(vec);
        assert_eq!(weak.strong_count(), 0);
    }
}
