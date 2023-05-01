use std::{sync::atomic::{AtomicPtr, Ordering, AtomicUsize}, ops::{Deref, DerefMut}, mem::take};


struct ANode<T> {
    value: T,
    next: Option<Box<Self>>
}


/// An unbounded, non-empty, concurrently appendable linked list
/// 
/// This vector uses an atomic pointer and a counter to allow
/// pushes to be done through immutable references
pub struct ALinkedList<T> {
    first: Box<ANode<T>>,
    last: AtomicPtr<ANode<T>>,
    len: AtomicUsize
}


impl<T> ALinkedList<T> {
    pub fn new(value: T) -> Self {
        let mut node = Box::new(ANode {
            value,
            next: None,
        });
        Self {
            last: AtomicPtr::new(node.as_mut()),
            first: node,
            len: AtomicUsize::new(1)
        }
    }

    pub fn push_back(&self, value: T) {
        let mut next = Box::new(
            ANode {
                value,
                next: None
            }
        );

        let last_ptr = self.last.swap(next.as_mut(), Ordering::Relaxed);
        self.len.fetch_add(1, Ordering::Relaxed);

        unsafe {
            last_ptr.as_mut().unwrap_unchecked().next = Some(next);
        }
    }

    pub fn push_front(&mut self, value: T) {
        let next = unsafe { std::ptr::read(&self.first) };

        let first = Box::new(
            ANode {
                value,
                next: Some(next)
            }
        );

        self.first = first;
    }

    #[inline]
    pub fn first(&self) -> &T {
        &self.first.value
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.len.load(Ordering::Relaxed)
    }

    #[inline]
    pub fn iter(&self) -> ALinkedListIter<T> {
        self.into_iter()
    }

    #[inline]
    pub fn iter_mut(&mut self) -> ALinkedListIterMut<T> {
        self.into_iter()
    }
}


pub struct ALinkedListIter<'a, T> {
    current_node: Option<&'a ANode<T>>
}


impl<'a, T> Iterator for ALinkedListIter<'a, T> {
    type Item = &'a T;

    fn next(&mut self) -> Option<Self::Item> {
        let node_ref = self.current_node?;
        self.current_node = node_ref.next.as_ref().map(Deref::deref);
        Some(&node_ref.value)
    }
}


impl<'a, T> IntoIterator for &'a ALinkedList<T> {
    type Item = &'a T;

    type IntoIter = ALinkedListIter<'a, T>;

    fn into_iter(self) -> Self::IntoIter {
        ALinkedListIter {
            current_node: Some(self.first.deref())
        }
    }
}


pub struct ALinkedListIterMut<'a, T> {
    current_node: Option<&'a mut ANode<T>>
}


impl<'a, T> Iterator for ALinkedListIterMut<'a, T> {
    type Item = &'a mut T;

    fn next(&mut self) -> Option<Self::Item> {
        let node_ref = take(&mut self.current_node)?;
        self.current_node = node_ref.next.as_mut().map(DerefMut::deref_mut);
        Some(&mut node_ref.value)
    }
}


impl<'a, T> IntoIterator for &'a mut ALinkedList<T> {
    type Item = &'a mut T;

    type IntoIter = ALinkedListIterMut<'a, T>;

    fn into_iter(self) -> Self::IntoIter {
        ALinkedListIterMut {
            current_node: Some(self.first.deref_mut())
        }
    }
}

