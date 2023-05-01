use std::{ptr::{NonNull}, sync::Arc, slice::Iter, mem::transmute};

use concurrent_queue::ConcurrentQueue;

const BASE_BLOCK_SIZE: usize = 32;
const BLOCK_GROWTH_RATE: usize = 2;


/// A 'drop' reference into an MVec
/// 
/// While an `MItemRef` does not currently provide any
/// way to access its associated item, dropping this struct
/// queues the associated item for dropping
/// (it will never be dropped immediately)
pub struct MItemRef<T> {
    ptr: NonNull<T>,
    pending_drops: Arc<ConcurrentQueue<NonNull<T>>>
}


/// A staged push and deletion queue
/// 
/// Pushes and deletions are queued and never done immediately.
/// Thus, these operations can be performed while iterating through the vector.
/// 
/// The allocation strategy used by this vector is optimized for rapid growth.
/// If the backing capacity is filled, new capacity is allocated but old elements
/// *are not* copied over.
pub struct MVec<T, K = usize> {
    blocks: Vec<Vec<T>>,
    pending_drops: Arc<ConcurrentQueue<NonNull<T>>>,
    pending_pushes: ConcurrentQueue<(K, T)>,
    last_nonfull_block_index: usize
}


impl<T, K> MVec<T, K> {
    pub fn new() -> Self {
        Self {
            blocks: vec![Vec::with_capacity(BASE_BLOCK_SIZE)],
            pending_drops: Arc::new(ConcurrentQueue::unbounded()),
            pending_pushes: ConcurrentQueue::unbounded(),
            last_nonfull_block_index: 0
        }
    }

    pub fn queue_push(&self, value: T, identifier: K) {
        assert!(self.pending_pushes.push((identifier, value)).is_ok());
    }

    fn get_first_nonfull_block(&mut self) -> &mut Vec<T> {
        unsafe { self.blocks.get_unchecked_mut(self.last_nonfull_block_index) }
    }

    fn get_last_nonempty_block(&mut self) -> &mut Vec<T> {
        let mut i = self.last_nonfull_block_index;

        loop {
            let last_mut: &mut Vec<T> = unsafe { transmute(self.blocks.get_unchecked_mut(i)) };
            if last_mut.len() > 0 {
                return last_mut
            }
            i -= 1;
        }
    }

    fn push_back(&mut self, value: T, identifier: K, out: &mut Vec<(K, MItemRef<T>)>) {
        let last_mut = self.get_first_nonfull_block();
        if let Err(value) = last_mut.push_within_capacity(value) {
            let mut vec = Vec::with_capacity(last_mut.len() * BLOCK_GROWTH_RATE);
            vec.push(value);

            out.push((identifier, MItemRef { ptr: vec.last().unwrap().into(), pending_drops: self.pending_drops.clone() }));

            self.blocks.push(vec);
            self.last_nonfull_block_index += 1;
        }
    }

    /// Adds and removes items that are queued for either action.
    /// 
    /// `out` is a mutable reference to a vector that will be filled in
    /// by `MItemRef` to newly added items
    pub fn process_queue(&mut self, out: &mut Vec<(K, MItemRef<T>)>) {
        // Push into free spots
        while let Ok((id, push)) = self.pending_pushes.pop() {
            let Ok(mut spot) = self.pending_drops.pop() else {
                self.push_back(push, id, out);

                while let Ok((id, push)) = self.pending_pushes.pop() {
                    self.push_back(push, id, out);
                }

                return
            };

            unsafe { *spot.as_mut() = push; }
            out.push((id, MItemRef { ptr: spot, pending_drops: self.pending_drops.clone() }));
        }

        // Drop all remaining free spots
        // by swapping with end elements
        // and then popping
        while let Ok(spot) = self.pending_drops.pop() {
            let last_mut = self.get_last_nonempty_block();
            let end_mut = unsafe { last_mut.last_mut().unwrap_unchecked() };
            
            unsafe {
                std::ptr::swap(spot.as_ptr(), end_mut);
            }
            last_mut.pop();
        }
    }

    pub fn iter(&self) -> MVecIter<T> {
        self.into_iter()
    }

    // pub fn iter_mut(&mut self) -> MVecIterMut<T> {
    //     self.into_iter()
    // }
}

impl<T> Drop for MItemRef<T> {
    fn drop(&mut self) {
        self.pending_drops.push(self.ptr).unwrap();
    }
}


impl<T> MItemRef<T> {
    /// Forgets self while minimizing memory leaks
    /// 
    /// This struct holds at least one `Arc`, so using `mem::forget` can
    /// cause the memory held by that `Arc` to leak. If you wish to persist
    /// the data pointed to by this `MItemRef`, you should use this method instead
    pub fn forget(mut self) {
        unsafe {
            std::ptr::drop_in_place(&mut self.pending_drops);
        }
        std::mem::forget(self);
    }
}


pub struct MVecIter<'a, T> {
    blocks_iter: Iter<'a, Vec<T>>,
    block_iter: Option<Iter<'a, T>>
}


impl<'a, T> Iterator for MVecIter<'a, T> {
    type Item = &'a T;

    fn next(&mut self) -> Option<Self::Item> {
        if let Some(x) = self.block_iter.as_mut()?.next() {
            Some(x)
        } else if let Some(x) = self.blocks_iter.next() {
            self.block_iter = Some(x.into_iter());
            self.next()
        } else {
            None
        }
    }
}


impl<'a, T, K> IntoIterator for &'a MVec<T, K> {
    type Item = &'a T;

    type IntoIter = MVecIter<'a, T>;

    fn into_iter(self) -> Self::IntoIter {
        let mut blocks_iter = self.blocks.iter();
        MVecIter {
            block_iter: blocks_iter.next().map(IntoIterator::into_iter),
            blocks_iter,
        }
    }
}
