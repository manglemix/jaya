use std::{cell::{SyncUnsafeCell}, mem::{MaybeUninit, transmute}, sync::{atomic::{AtomicUsize, Ordering}}, ptr::NonNull};

use crossbeam::atomic::AtomicConsume;


/// A bounded, concurrently appendable vector
/// 
/// This vector uses an atomic counter to allow pushes to be possible
/// through immutable references
pub struct AVec<T> {
    len: AtomicUsize,
    rawvec: Box<[SyncUnsafeCell<MaybeUninit<T>>]>,
}


impl<T> AVec<T> {
    pub fn new(capacity: usize) -> Self {
        Self {
            len: AtomicUsize::default(),
            rawvec: {
                let mut rawvec = Vec::with_capacity(capacity);
                for _i in 0..capacity {
                    rawvec.push(SyncUnsafeCell::new(MaybeUninit::uninit()));
                }
                rawvec.into_boxed_slice()
            }
        }
    }

    /// Tries to push the given value into the vector
    /// 
    /// Returns `Ok` with a `NonNull` ptr to the item, which
    /// is *always* valid unless the vector is dropped.
    /// Returns `Err` if there is no more space (all subsequent
    /// pushes will also fail)
    /// 
    /// Each `push` involves exactly 1 atomic `fetch_add`
    pub fn push(&self, value: T) -> Result<NonNull<T>, T> {
        let len = self.len.fetch_add(1, Ordering::Acquire);

        debug_assert!(len <= self.rawvec.len());
        if len == self.rawvec.len() {
            self.len.fetch_sub(1, Ordering::Acquire);
            return Err(value)
        }

        unsafe {
            Ok(
                self
                    .rawvec
                    .get_unchecked(len)
                    .get()
                    .as_mut()
                    .unwrap_unchecked()
                    .write(value)
                    .into()
            )
        }
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.len.load_consume()
    }

    pub fn get(&self, index: usize) -> Option<&T> {
        if self.len() > index {
            unsafe {
                Some(
                    self.rawvec
                        .get_unchecked(index)
                        .get()
                        .as_ref()
                        .unwrap()
                        .assume_init_ref()
                )
            }
        } else {
            None
        }
    }

    pub unsafe fn get_unchecked(&self, index: usize) -> &T {
        self.rawvec
            .get_unchecked(index)
            .get()
            .as_ref()
            .unwrap()
            .assume_init_ref()
    }

    pub fn get_mut(&mut self, index: usize) -> Option<&mut T> {
        if self.len.load_consume() > index {
            unsafe {
                Some(
                    self.rawvec
                        .get_unchecked(index)
                        .get()
                        .as_mut()
                        .unwrap_unchecked()
                        .assume_init_mut()
                )
            }
        } else {
            None
        }
    }

    pub fn get_all_mut(&mut self) -> Box<[&mut T]> {
        let len = self.len();
        let mut refs = Vec::with_capacity(len);

        for i in 0..len {
            refs.push(unsafe {
                self
                    .rawvec
                    .get_unchecked(i)
                    .get()
                    .as_mut()
                    .unwrap_unchecked()
                    .assume_init_mut()
            });
        }

        refs.into_boxed_slice()
    }

    pub fn iter(&self) -> AVecIter<T> {
        self.into_iter()
    }

    pub fn iter_mut(&mut self) -> AVecIterMut<T> {
        self.into_iter()
    }
}

impl<T> Drop for AVec<T> {
    fn drop(&mut self) {
        for i in 0..*self.len.get_mut() {
            unsafe {
                self.rawvec
                    .get_unchecked_mut(i)
                    .get_mut()
                    .assume_init_drop()
            };
        }
    }
}


pub struct AVecIter<'a, T> {
    vec_ref: &'a AVec<T>,
    counter: usize
}


impl<'a, T> Iterator for AVecIter<'a, T> {
    type Item = &'a T;

    fn next(&mut self) -> Option<Self::Item> {
        if self.counter >= self.vec_ref.len.load(Ordering::Acquire) {
            None
        } else {
            let out = unsafe {
                self
                    .vec_ref
                    .rawvec[self.counter]
                    .get()
                    .as_ref()
                    .unwrap_unchecked()
                    .assume_init_ref()
            };
            self.counter += 1;
            Some(out)
        }
    }
}


impl<'a, T> IntoIterator for &'a AVec<T> {
    type Item = &'a T;

    type IntoIter = AVecIter<'a, T>;

    fn into_iter(self) -> Self::IntoIter {
        AVecIter {
            vec_ref: self,
            counter: 0,
        }
    }
    
}


pub struct AVecIterMut<'a, T> {
    vec_ref: &'a mut AVec<T>,
    counter: usize
}


impl<'a, T> Iterator for AVecIterMut<'a, T> {
    type Item = &'a mut T;

    fn next(&mut self) -> Option<Self::Item> {
        if self.counter >= self.vec_ref.len.load(Ordering::Acquire) {
            None
        } else {
            let out = unsafe {
                let out = self
                    .vec_ref
                    .rawvec
                    .get_mut(self.counter)
                    .unwrap_unchecked()
                    .get_mut()
                    .assume_init_mut();

                transmute(out)
            };
            self.counter += 1;
            Some(out)
        }
    }
}


impl<'a, T> IntoIterator for &'a mut AVec<T> {
    type Item = &'a mut T;

    type IntoIter = AVecIterMut<'a, T>;

    fn into_iter(self) -> Self::IntoIter {
        AVecIterMut {
            vec_ref: self,
            counter: 0,
        }
    }
    
}


#[cfg(test)]
mod tests {
    use rayon::prelude::{IntoParallelIterator, ParallelIterator};

    use super::*;

    #[test]
    fn many_pushes() {
        let avec = AVec::<u16>::new(100);

        (0..100).into_par_iter()
            .for_each(|n| {
                assert!(avec.push(n).is_ok());
            });
        assert_eq!(avec.push(100), Err(100));
    }
}
