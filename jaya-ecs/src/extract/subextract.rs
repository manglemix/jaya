use std::{
    hash::Hasher,
    num::NonZeroUsize,
    ptr::NonNull,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{sync_channel, Receiver, SyncSender},
        Arc,
    },
    thread::{available_parallelism, current, park, Thread},
};

use crossbeam::queue::SegQueue;
use fxhash::FxHashSet;
use hashers::jenkins::spooky_hash::SpookyHasher;
use rayon::{
    prelude::{IntoParallelRefMutIterator, ParallelDrainRange, ParallelIterator},
    spawn,
};

use crate::component::Component;

use super::Query;

const DEFAULT_MODIFIER_BUFFER_SIZE: usize = 1_000;
const MULTI_MODIFIER_LIMIT: usize = 4;
type PtrArray = [Option<NonNull<()>>; MULTI_MODIFIER_LIMIT];

pub(crate) struct ComponentModifier {
    pub(super) ptr: NonNull<()>,
    pub(super) f: Box<dyn FnOnce(NonNull<()>)>,
}

// impl std::hash::Hash for ComponentModifier {
//     fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
//         self.ptr.hash(state);
//     }
// }

impl PartialEq for ComponentModifier {
    fn eq(&self, other: &Self) -> bool {
        self.ptr == other.ptr
    }
}

// impl PartialOrd for ComponentModifier {
//     fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
//         self.ptr.partial_cmp(&other.ptr)
//     }
// }

impl Eq for ComponentModifier {}

// impl Ord for ComponentModifier {
//     fn cmp(&self, other: &Self) -> std::cmp::Ordering {
//         self.ptr.cmp(&other.ptr)
//     }
// }

impl ComponentModifier {
    pub unsafe fn execute(self) {
        (self.f)(self.ptr);
    }
}

unsafe impl Send for ComponentModifier {}
unsafe impl Sync for ComponentModifier {}

/// A simplified separate-chaining hash table for separating modifiers into chains that can be executed in parallel
///
/// A `ComponentModifier` is hashed by its pointer to a `Component` and thus will always be placed in the same chain.
/// More often than not, modifiers to different components will be stored in the same chain, but that is still safe.
/// There will always be as many chains as threads (roughly), so there is no benefit to storing modifiers in more separate
/// chains in terms of performance
pub struct ComponentModifierStager {
    modifiers: Box<[SegQueue<ComponentModifier>]>,
}

impl Default for ComponentModifierStager {
    fn default() -> Self {
        Self::new()
    }
}

impl ComponentModifierStager {
    pub fn new() -> Self {
        let parallel_count = available_parallelism()
            .unwrap_or(NonZeroUsize::new(8).unwrap())
            .get();
        let mut modifiers = Vec::with_capacity(parallel_count);

        for _i in 0..parallel_count {
            modifiers.push(SegQueue::default());
        }

        Self {
            modifiers: modifiers.into_boxed_slice(),
        }
    }

    pub(crate) fn queue_modifier(&self, modifier: ComponentModifier) {
        let mut hash = SpookyHasher::default();
        hash.write_usize(modifier.ptr.as_ptr() as usize);
        let hash = hash.finish() as usize;

        self.modifiers
            .get(hash % self.modifiers.len())
            .unwrap()
            .push(modifier);
    }

    /// # Safety
    /// There must be no references to any `Component` which has queued modification
    pub unsafe fn execute(&mut self) {
        // println!("{:?}", self.modifiers.iter().map(|x| x.len()).collect::<Vec<_>>());
        self.modifiers.par_iter_mut().for_each(|queue| {
            while let Some(modifier) = queue.pop() {
                modifier.execute();
            }
        });
    }
}

#[derive(Default)]
struct MultiModificationStage {
    ptrs: FxHashSet<usize>,
    modifiers: Vec<MultiComponentModifier>,
}

enum MultiEnqueue {
    Modifier(MultiComponentModifier),
    Close(Thread),
    Resize(Receiver<Self>),
}

/// A data structure for sorting modifiers that require multiple mutable references
///
/// This uses an Actor to sort concurrently, so requires far more resources than `ComponentModifierStager`.
/// Each `Component` pointer in a multi modifier is hashed, and its presence is checked in a hash table.
/// If all pointers are not found in the hash table, then the modifier can be executed in parallel with the
/// other modifiers in that table. Otherwise, another table is used (or made if there isn't one) and the process
/// is repeated. Thus, all modifiers in a table can be executed in parallel, but tables must be ran sequentially
pub struct MultiComponentModifierStager {
    modification_complete: Arc<AtomicBool>,
    mod_sender: SyncSender<MultiEnqueue>,
    buffer_size: usize,
    buffer_filled: AtomicBool,
}

impl MultiComponentModifierStager {
    pub fn new() -> Self {
        let (mod_sender, mut mod_recv) = sync_channel(DEFAULT_MODIFIER_BUFFER_SIZE);
        let modification_complete: Arc<AtomicBool> = Default::default();
        let mod_complete_cloned = modification_complete.clone();

        spawn(move || {
            let mut stages: Vec<MultiModificationStage> = Default::default();

            'b: loop {
                let Ok(x) = mod_recv.recv() else { break };
                let modifier = match x {
                    MultiEnqueue::Resize(new_recv) => {
                        mod_recv = new_recv;
                        continue;
                    }
                    MultiEnqueue::Modifier(x) => x,
                    MultiEnqueue::Close(thr) => {
                        // println!("{}", stages.iter().map(|x| x.modifiers.len()).sum::<usize>());
                        stages.iter_mut().for_each(|stage| {
                            stage.ptrs.clear();
                            stage.modifiers.par_drain(..).for_each(|x| unsafe {
                                x.execute();
                            });
                        });

                        mod_complete_cloned.store(true, Ordering::Release);
                        thr.unpark();
                        continue;
                    }
                };

                'a: for stage in stages.iter_mut() {
                    for ptr in modifier.ptrs {
                        let Some(ptr) = ptr else {
                            break
                        };
                        let ptr = ptr.as_ptr() as usize;
                        if stage.ptrs.contains(&ptr) {
                            continue 'a;
                        }
                    }
                    stage.ptrs.extend(
                        modifier
                            .ptrs
                            .into_iter()
                            .map_while(|x| x)
                            .map(|x| x.as_ptr() as usize),
                    );
                    stage.modifiers.push(modifier);
                    continue 'b;
                }
                let mut stage = MultiModificationStage::default();
                stage.ptrs.extend(
                    modifier
                        .ptrs
                        .into_iter()
                        .map_while(|x| x)
                        .map(|x| x.as_ptr() as usize),
                );
                stage.modifiers.push(modifier);
                stages.push(stage);
            }
        });

        Self {
            mod_sender,
            modification_complete,
            buffer_size: DEFAULT_MODIFIER_BUFFER_SIZE,
            buffer_filled: Default::default(),
        }
    }

    pub(crate) fn queue_modifier(&self, modifier: MultiComponentModifier) {
        unsafe {
            let Err(err) = self.mod_sender.try_send(MultiEnqueue::Modifier(modifier)) else { return };
            self.buffer_filled.store(true, Ordering::Relaxed);
            match err {
                std::sync::mpsc::TrySendError::Full(modifier) => {
                    self.mod_sender.send(modifier).unwrap_unchecked()
                }
                _ => std::hint::unreachable_unchecked(),
            }
        }
    }

    pub unsafe fn execute(&mut self) {
        self.mod_sender
            .send(MultiEnqueue::Close(current()))
            .unwrap_unchecked();

        while !self.modification_complete.load(Ordering::Relaxed) {
            park();
        }
        self.modification_complete.store(false, Ordering::Relaxed);

        if self.buffer_filled.load(Ordering::Relaxed) {
            self.buffer_size = self.buffer_size.saturating_mul(2);
            let (mod_sender, mod_recv) = sync_channel(self.buffer_size);

            self.mod_sender
                .send(MultiEnqueue::Resize(mod_recv))
                .unwrap_unchecked();
            self.mod_sender = mod_sender;

            self.buffer_filled.store(false, Ordering::Relaxed);
        }
    }
}

impl Default for MultiComponentModifierStager {
    fn default() -> Self {
        Self::new()
    }
}

pub(crate) struct MultiComponentModifier {
    ptrs: PtrArray,
    f: Box<dyn FnOnce(PtrArray)>,
}

unsafe impl Send for MultiComponentModifier {}
unsafe impl Sync for MultiComponentModifier {}

impl MultiComponentModifier {
    pub unsafe fn execute(self) {
        (self.f)(self.ptrs)
    }
}

/// A MultiQuery is simply the combination of multiple queries,
/// allowing for modifiers to have access to multiple mutable references to `Component`s at a time
pub trait MultiQuery {
    type Arguments;

    fn queue_mut(self, f: impl FnOnce(Self::Arguments) + 'static);
}

impl<'a, C1, C2> MultiQuery for (Query<'a, C1>, Query<'a, C2>)
where
    C1: Component + 'a,
    C2: Component + 'a,
{
    type Arguments = (&'a mut C1, &'a mut C2);

    fn queue_mut(self, f: impl FnOnce(Self::Arguments) + 'static) {
        self.0
            .universe
            .queue_multi_component_modifier(MultiComponentModifier {
                ptrs: [
                    Some(NonNull::from(self.0.component).cast()),
                    Some(NonNull::from(self.1.component).cast()),
                    None,
                    None,
                ],
                f: Box::new(|[ptr1, ptr2, _, _]| unsafe {
                    let mut ptr1 = ptr1.unwrap_unchecked().cast::<C1>();
                    let mut ptr2 = ptr2.unwrap_unchecked().cast::<C2>();
                    (f)((ptr1.as_mut(), ptr2.as_mut()))
                }),
            })
    }
}
