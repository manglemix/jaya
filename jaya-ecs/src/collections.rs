use std::{sync::{atomic::{AtomicBool, AtomicUsize, Ordering}, Arc}, any::TypeId, mem::{size_of, forget, MaybeUninit}, ptr::copy_nonoverlapping, marker::PhantomData, ops::Deref, hint::unreachable_unchecked};

use crossbeam::utils::Backoff;
use parking_lot::{RwLock, RwLockReadGuard};
use rayon::{prelude::{IntoParallelRefIterator, ParallelIterator, IndexedParallelIterator}, slice::ParallelSlice};


const DEFAULT_BLOCK_SIZE: usize = 32;
const BLOCK_GROWTH_RATE: usize = 2;


pub struct AnyVec {
    bytes: RwLock<Vec<Box<[u8]>>>,
    maybe_init_len: AtomicUsize,
    init_len: AtomicUsize,
    resizing: AtomicBool,
    mut_iterating: AtomicBool,
    type_id: TypeId
}


impl AnyVec {
    pub fn new<T: 'static>() -> Self {
        Self::with_capacity::<T>(DEFAULT_BLOCK_SIZE)
    }

    pub fn with_capacity<T: 'static>(capacity: usize) -> Self {
        let block = vec![0u8; capacity * size_of::<T>()];
        Self {
            bytes: RwLock::new(vec![block.into_boxed_slice()]),
            resizing: Default::default(),
            maybe_init_len: Default::default(),
            type_id: TypeId::of::<T>(),
            init_len: Default::default(),
            mut_iterating: Default::default()
        }
    }

    pub fn get<T: 'static>(&self, index: usize) -> Option<&T> {
        if TypeId::of::<T>() != self.type_id {
            return None
        }

        if index >= self.init_len.load(Ordering::Relaxed) {
            return None
        }
        
        unsafe {
            let ptr = AnyVec::get_bytes_ptr::<T>(index, &self.bytes.read());
            Some(&(*ptr.cast()))
        }
    }

    #[must_use]
    pub fn push<T: Send + Sync + 'static>(&self, value: T) -> Option<T> {
        if TypeId::of::<T>() != self.type_id {
            return Some(value)
        }

        let t_size = size_of::<T>();
        let t_ptr: *const T = &value;
        let t_ptr: *const u8 = t_ptr.cast();
        let current_len = self.maybe_init_len.fetch_add(1, Ordering::Acquire) + 1;

        loop {
            let reader = self.bytes.read();

            if current_len >= Self::get_capacity::<T>(reader.deref()) {
                let last_resizing_value = self.resizing.swap(true, Ordering::Acquire);
                if last_resizing_value {
                    // another thread is already resizing
                    RwLockReadGuard::unlock_fair(reader);
                    continue
                }
                let new_block = vec![0u8; unsafe { reader.last().unwrap_unchecked().len() * BLOCK_GROWTH_RATE }];
                drop(reader);
                self
                    .bytes
                    .write()
                    .push(new_block.into_boxed_slice());
                self.resizing.store(false, Ordering::Release);
                continue
            }

            unsafe {
                let start_ptr = Self::get_bytes_ptr::<T>(current_len - 1, &reader);
                copy_nonoverlapping(t_ptr, start_ptr, t_size);
            }
            forget(value);

            let backoff = Backoff::new();
            while self.init_len.load(Ordering::Relaxed) != current_len - 1 {
                backoff.snooze();
            }
            self.init_len.fetch_add(1, Ordering::Relaxed);

            break
        }

        None
    }

    pub fn par_iter<'a, T, F>(&'a self, f: F) -> bool
    where
        T: 'static,
        F: Fn(&T) + Sync
    {
        if self.mut_iterating.load(Ordering::Relaxed) {
            return false
        }
        self.bytes
            .read()
            .par_iter()
            .for_each(|block| {
                block.par_chunks_exact(size_of::<T>())
                    .for_each(|ptr| unsafe {
                        let ptr: *const u8 = ptr.get(0).unwrap();
                        let ptr: *const T = ptr.cast();
                        (f)(ptr.as_ref().unwrap())
                    });
            });
        true
    }

    pub fn par_iter_combinations<'a, T, F, const N: usize>(&'a self, f: F) -> bool
    where
        T: Sync + 'static,
        F: Fn([&T; N]) + Sync
    {
        if self.mut_iterating.load(Ordering::Relaxed) {
            return false
        }
        if N == 0 {
            return true
        }
        let init_len = self.init_len.load(Ordering::Relaxed);
        self.bytes
            .read()
            .par_iter()
            .enumerate()
            .map(|(i, block)| {
                let start_index = (block.len() as f64 - 0.5f64.powi(i.try_into().unwrap())) as usize;
                block.par_chunks_exact(size_of::<T>())
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
                        let ptr = AnyVec::get_bytes_ptr::<T>(j, &self.bytes.read());
                        arr[1].write(&(*ptr.cast()));
                    }
                    (f)(arr.map(|x| unsafe { x.assume_init() }))
                }
            });
        true
    }

    pub fn iter<T: 'static>(&self) -> Option<AnyVecIter<T>> {
        if self.mut_iterating.load(Ordering::Relaxed) {
            return None
        }
        Some(AnyVecIter {
                    len: if TypeId::of::<T>() == self.type_id {
                        self.init_len.load(Ordering::Relaxed)
                    } else {
                        0
                    },
                    current_index: 0,
                    vec_ref: Arc::new(self.bytes.read()),
                    _phantom: Default::default()
                })
    }

    pub fn iter_mut<T: 'static>(&self) -> Option<AnyVecIterMut<T>> {
        if self.mut_iterating.swap(true, Ordering::Relaxed) {
            return None
        }
        Some(AnyVecIterMut {
                    len: if TypeId::of::<T>() == self.type_id {
                        self.init_len.load(Ordering::Relaxed)
                    } else {
                        0
                    },
                    current_index: 0,
                    vec_ref: self.bytes.read(),
                    _phantom: Default::default(),
                    mut_iterating: &self.mut_iterating,
                })
    }

    pub fn exclusive_iter_mut<T: 'static>(&mut self) -> AnyVecIterMut<T> {
        debug_assert!(!self.mut_iterating.load(Ordering::Relaxed));
        self.mut_iterating.store(true, Ordering::Relaxed);
        AnyVecIterMut {
            len: if TypeId::of::<T>() == self.type_id {
                self.init_len.load(Ordering::Relaxed)
            } else {
                0
            },
            current_index: 0,
            vec_ref: self.bytes.read(),
            _phantom: Default::default(),
            mut_iterating: &self.mut_iterating,
        }
    }

    #[must_use]
    pub fn drop<T: 'static>(mut self) -> Option<Self> {
        if TypeId::of::<T>() != self.type_id {
            return Some(self)
        }
        self
            .exclusive_iter_mut::<T>()
            .for_each(|x| unsafe {
                std::ptr::drop_in_place(x);
            });
        None
    }

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
                return x.cast_mut()
            }
            index -= block.len();
        }
        unreachable_unchecked()
    }
}


pub struct AnyVecIter<'a, T> {
    vec_ref: Arc<RwLockReadGuard<'a, Vec<Box<[u8]>>>>,
    len: usize,
    current_index: usize,
    _phantom: PhantomData<T>
}


impl<'a, T: 'a> Iterator for AnyVecIter<'a, T> {
    type Item = &'a T;

    fn next(&mut self) -> Option<Self::Item> {
        if self.current_index >= self.len {
            return None
        }
        
        unsafe {
            let ptr = AnyVec::get_bytes_ptr::<T>(self.current_index, &self.vec_ref);
            self.current_index += 1;
            Some(&(*ptr.cast()))
        }
    }
}


pub struct AnyVecIterMut<'a, T> {
    vec_ref: RwLockReadGuard<'a, Vec<Box<[u8]>>>,
    len: usize,
    current_index: usize,
    mut_iterating: &'a AtomicBool,
    _phantom: PhantomData<T>
}


impl<'a, T: 'a> Iterator for AnyVecIterMut<'a, T> {
    type Item = &'a mut T;

    fn next(&mut self) -> Option<Self::Item> {
        if self.current_index >= self.len {
            return None
        }
        
        unsafe {
            let ptr = AnyVec::get_bytes_ptr::<T>(self.current_index, &self.vec_ref);
            self.current_index += 1;
            Some(&mut (*ptr.cast()))
        }
    }
}

impl<'a, T> Drop for AnyVecIterMut<'a, T> {
    fn drop(&mut self) {
        self.mut_iterating.store(false, Ordering::Relaxed);
    }
}


#[cfg(test)]
mod tests {
    use std::{collections::HashSet, sync::Arc, ops::Deref};

    use itertools::Itertools;
    use rayon::prelude::{IntoParallelIterator, ParallelIterator};

    use super::AnyVec;

    #[test]
    fn test01() {
        let vec = AnyVec::new::<usize>();

        (0..500usize)
            .into_iter()
            .for_each(|n| {
                assert_eq!(vec.push(n), None);
            });

        assert_eq!(vec.push(13u8), Some(13u8));
        assert_eq!(vec.iter::<usize>().unwrap().cloned().collect_vec(), (0..500).into_iter().collect_vec());
        assert_eq!(vec.iter::<i8>().unwrap().next(), None);
    }
    #[test]
    fn par_test01() {
        let vec = AnyVec::new::<usize>();

        (0..500usize)
            .into_par_iter()
            .for_each(|n| {
                assert_eq!(vec.push(n), None);
            });

        assert_eq!(vec.push(13u8), Some(13u8));
        assert_eq!(vec.iter::<usize>().unwrap().cloned().collect::<HashSet<usize>>(), (0..500).into_iter().collect::<HashSet<usize>>());
        assert_eq!(vec.iter::<i8>().unwrap().next(), None);
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
        assert!(vec.drop::<Arc<(u8, u64, i128)>>().is_none());
        assert_eq!(weak.strong_count(), 0);
    }
}
