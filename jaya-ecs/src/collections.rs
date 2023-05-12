use std::{
    any::TypeId,
    cell::SyncUnsafeCell,
    fmt::Debug,
    hint::unreachable_unchecked,
    marker::PhantomData,
    mem::{forget, size_of, MaybeUninit},
    num::NonZeroUsize,
    ptr::{copy_nonoverlapping, drop_in_place},
    sync::atomic::{AtomicBool, AtomicUsize, Ordering},
};

use crossbeam::utils::Backoff;
use rayon::{
    prelude::{IndexedParallelIterator, IntoParallelRefIterator, ParallelIterator},
    slice::ParallelSlice,
};

#[derive(derive_more::From, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PushError<T>(pub T);

impl<T> Debug for PushError<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Incorrect type on push to AnyVec")
    }
}

const DEFAULT_BLOCK_SIZE: usize = 32;
const BLOCK_GROWTH_RATE: usize = 2;

/// A vector of elements whose type is not statically verified (notice no generic parameter for the type)
///
/// On top of this feature, `AnyVec` can be extended and iterated through concurrently, and removals can happen concurrently (but not all three at once)
pub(crate) struct AnyVec {
    bytes: SyncUnsafeCell<Vec<SyncUnsafeCell<Box<[u8]>>>>,
    maybe_init_len: AtomicUsize,
    init_len: AtomicUsize,
    resizing: AtomicBool,
    type_id: TypeId,
    dropper: fn(*mut u8),
    element_size: usize,
}

impl AnyVec {
    pub fn new<T: 'static>() -> Self {
        Self::with_capacity::<T>(DEFAULT_BLOCK_SIZE.try_into().unwrap())
    }

    pub fn with_capacity<T: 'static>(capacity: NonZeroUsize) -> Self {
        let capacity = capacity.get();
        let block = vec![0u8; capacity * size_of::<T>()];
        Self {
            bytes: SyncUnsafeCell::new(vec![SyncUnsafeCell::new(block.into_boxed_slice())]),
            resizing: Default::default(),
            maybe_init_len: Default::default(),
            type_id: TypeId::of::<T>(),
            init_len: Default::default(),
            dropper: |ptr| unsafe {
                drop_in_place::<T>(ptr.cast());
            },
            element_size: size_of::<T>(),
        }
    }

    /// # Safety
    /// Behaviour is undefined if any of the following isn't true
    /// 1. `ptr` must be a pointer inside of `self`
    /// 2. `size` must be the actual size of the type stored in `self`
    /// 3. `self` must not be empty
    /// 4. There must not be any thread that is concurrently adding elements
    /// 5. `ptr` must not currently be used. This includes calls to this method. (ie. Do not use the same `ptr` to this method concurrently)
    /// 6. `ptr` must point to a valid object that is of the same type that `self` is initialized for
    ///
    /// # Programmer's Note
    /// This is probably the most unsafe function I've written
    pub unsafe fn swap_remove_by_ptr(&self, ptr: *mut u8) -> bool {
        (self.dropper)(ptr);
        let current_len = self.init_len.fetch_sub(1, Ordering::Relaxed) - 1;
        self.maybe_init_len.fetch_sub(1, Ordering::Relaxed);
        let last_ptr = self.get_bytes_ptr_manually(current_len, self.element_size);
        if last_ptr == ptr {
            return false;
        }
        copy_nonoverlapping(last_ptr, ptr, self.element_size);
        true
    }

    // pub fn get<T: 'static>(&self, index: usize) -> Option<&T> {
    //     if TypeId::of::<T>() != self.type_id {
    //         return None;
    //     }

    //     if index >= self.init_len.load(Ordering::Relaxed) {
    //         return None;
    //     }

    //     unsafe {
    //         let ptr = AnyVec::get_bytes_ptr::<T>(index, &(*self.bytes.get()));
    //         Some(&(*ptr.cast()))
    //     }
    // }

    /// Pushes the given value onto the `self`
    ///
    /// If the type of the given value is the type that this vector was initialized for, then `Ok`
    /// is returned containing a bytes ptr to where the value is stored. Otherwise, `Err` is returned
    /// containing the given value.
    #[must_use]
    pub fn push<T: Send + Sync + 'static>(&self, value: T) -> Result<*mut u8, PushError<T>> {
        if TypeId::of::<T>() != self.type_id {
            return Err(value.into());
        }

        let t_size = size_of::<T>();
        let t_ptr: *const T = &value;
        let t_ptr: *const u8 = t_ptr.cast();
        let current_len = self.maybe_init_len.fetch_add(1, Ordering::Acquire) + 1;
        let backoff = Backoff::new();

        loop {
            let last_block_len;

            {
                let blocks = unsafe { &(*self.bytes.get()) };

                if current_len >= self.get_capacity::<T>() {
                    let last_resizing_value = self.resizing.swap(true, Ordering::Acquire);
                    if last_resizing_value {
                        // another thread is already resizing
                        backoff.snooze();
                        continue;
                    }
                    last_block_len = unsafe {
                        (*blocks.last().unwrap_unchecked().get()).len() * BLOCK_GROWTH_RATE
                    };
                } else {
                    let start_ptr;
                    unsafe {
                        start_ptr = self.get_bytes_ptr::<T>(current_len - 1);
                        copy_nonoverlapping(t_ptr, start_ptr, t_size);
                    }
                    forget(value);

                    backoff.reset();
                    while self.init_len.load(Ordering::Relaxed) != current_len - 1 {
                        backoff.snooze();
                    }
                    self.init_len.fetch_add(1, Ordering::Relaxed);
                    break Ok(start_ptr);
                }
            }

            let new_block = SyncUnsafeCell::new(vec![0u8; last_block_len].into_boxed_slice());

            unsafe {
                (&mut (*self.bytes.get())).push(new_block);
            }

            self.resizing.store(false, Ordering::Release);
        }
    }

    pub fn par_iter<'a, T>(&'a self) -> Option<impl ParallelIterator<Item = &'a T>>
    where
        T: Sync + 'static,
    {
        if TypeId::of::<T>() != self.type_id {
            return None;
        }
        let init_len = self.init_len.load(Ordering::Relaxed);
        unsafe {
            Some(
                (&(*self.bytes.get()))
                    .par_iter()
                    .enumerate()
                    .map(move |(i, block)| {
                        let block = &(*block.get());
                        let start_index = (block.len() as f64
                            * (1.0
                                - (1.0f64 / BLOCK_GROWTH_RATE.pow(i.try_into().unwrap()) as f64)))
                            as usize
                            / size_of::<T>();

                        block
                            .par_chunks_exact(size_of::<T>())
                            .enumerate()
                            .filter_map(move |(j, ptr)| {
                                if start_index + j >= init_len {
                                    return None;
                                }
                                let ptr: *const T = ptr.as_ptr().cast();
                                Some(&(*ptr))
                            })
                    })
                    .flatten(),
            )
        }
    }

    pub fn par_iter_combinations<'a, T, F, const N: usize>(&'a self, f: F) -> bool
    where
        T: Sync + 'static,
        F: Fn([&'a T; N]) + Sync,
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
                let block = unsafe { &(*block.get()) };
                let start_index = (block.len() as f64
                    * (1.0 - (1.0f64 / BLOCK_GROWTH_RATE.pow(i.try_into().unwrap()) as f64)))
                    as usize
                    / size_of::<T>();
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
                for j in (i + 1)..init_len {
                    let mut arr = MaybeUninit::<&T>::uninit_array::<N>();
                    arr[0].write(x);
                    unsafe {
                        let ptr = self.get_bytes_ptr::<T>(j);
                        arr[1].write(&(*ptr.cast()));
                    }
                    (f)(arr.map(|x| unsafe { x.assume_init() }))
                }
            });

        true
    }

    #[cfg(test)]
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
            vec_ref: self,
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

    fn get_capacity<T>(&self) -> usize {
        self.get_capacity_manual(size_of::<T>())
    }

    fn get_capacity_manual(&self, t_size: usize) -> usize {
        unsafe { &(*self.bytes.get()) }
            .into_iter()
            .map(|block| unsafe { (*block.get()).len() } / t_size)
            .sum()
    }

    unsafe fn get_bytes_ptr<T>(&self, index: usize) -> *mut u8 {
        self.get_bytes_ptr_manually(index, size_of::<T>())
    }

    unsafe fn get_bytes_ptr_manually(&self, mut index: usize, t_size: usize) -> *mut u8 {
        let blocks = unsafe { &(*self.bytes.get()) };

        debug_assert!(index < self.get_capacity_manual(t_size));
        index *= t_size;
        for block in blocks {
            let block = unsafe { &mut (*block.get()) };
            // debug_assert_eq!(block.len() % t_size, 0);
            if let Some(x) = block.get_mut(index) {
                return x;
            }
            index -= block.len();
        }
        unreachable_unchecked()
    }
}

impl Drop for AnyVec {
    fn drop(&mut self) {
        debug_assert_eq!(self.init_len.get_mut(), self.maybe_init_len.get_mut());
        let mut init_len = *self.init_len.get_mut();
        if init_len == 0 {
            return;
        }

        for block in self.bytes.get_mut() {
            let block = block.get_mut();
            let ptr = block.as_mut_ptr();

            for i in 0..(block.len() / self.element_size) {
                unsafe {
                    (self.dropper)(ptr.offset((self.element_size * i) as isize));
                }
                init_len -= 1;
                if init_len == 0 {
                    return;
                }
            }
        }
    }
}

pub struct AnyVecIter<'a, T> {
    vec_ref: &'a AnyVec,
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
            let ptr = self.vec_ref.get_bytes_ptr::<T>(self.current_index);
            self.current_index += 1;
            Some(&(*ptr.cast()))
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{collections::HashSet, ops::Deref, sync::Arc};

    use rayon::prelude::{IntoParallelIterator, ParallelIterator};

    use crate::collections::PushError;

    use super::AnyVec;

    #[test]
    fn test01() {
        let vec = AnyVec::new::<usize>();

        (0..500usize).into_iter().for_each(|n| {
            assert!(vec.push(n).is_ok());
        });

        assert_eq!(vec.push(13u8), Err(PushError(13u8)));
        assert_eq!(
            vec.iter::<usize>().cloned().collect::<Vec<_>>(),
            (0..500usize).into_iter().collect::<Vec<_>>()
        );
        assert_eq!(vec.iter::<i8>().next(), None);
    }

    #[test]
    fn par_test01() {
        let vec = AnyVec::new::<usize>();

        (0..500usize).into_par_iter().for_each(|n| {
            assert!(vec.push(n).is_ok());
        });

        assert_eq!(vec.push(13u8), Err(PushError(13u8)));
        assert_eq!(
            vec.iter::<usize>().cloned().collect::<HashSet<usize>>(),
            (0..500).into_iter().collect::<HashSet<usize>>()
        );
        assert_eq!(vec.iter::<i8>().next(), None);
    }

    #[test]
    fn arc_test01() {
        let vec = AnyVec::new::<Arc<(u8, u64, i128)>>();
        let arc = Arc::new((0u8, 2u64, 56i128));
        let weak = Arc::downgrade(&arc);
        assert!(vec.push(arc).is_ok());

        let arc = weak.upgrade().unwrap();
        assert_eq!(arc.deref(), &(0, 2, 56));
        drop(arc);
        assert_eq!(weak.strong_count(), 1);
        drop(vec);
        assert_eq!(weak.strong_count(), 0);
    }

    #[test]
    fn arc_test02() {
        let vec = AnyVec::new::<Arc<(u8, u64, i128)>>();
        let arc = Arc::new((0u8, 2u64, 56i128));
        let weak = Arc::downgrade(&arc);
        let arc_ptr = vec.push(arc).unwrap();

        let arc = weak.upgrade().unwrap();
        assert_eq!(arc.deref(), &(0, 2, 56));
        drop(arc);
        assert_eq!(weak.strong_count(), 1);
        unsafe { assert!(!vec.swap_remove_by_ptr(arc_ptr)) }
        assert_eq!(weak.strong_count(), 0);
    }
}
