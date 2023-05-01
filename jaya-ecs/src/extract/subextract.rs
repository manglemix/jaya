use std::{ptr::NonNull, sync::{Arc, mpsc::{sync_channel, SyncSender}}};

use concurrent_queue::ConcurrentQueue;
use fxhash::{FxHashSet};
use parking_lot::{Mutex};
use rayon::{prelude::{ParallelIterator, ParallelDrainRange, ParallelDrainFull}, spawn};

use crate::component::{Component, ComponentContainer, ComponentModifierContainer};

use super::{Query};

const MULTI_MODIFIER_LIMIT: usize = 4;
type PtrArray = [Option<NonNull<()>>; MULTI_MODIFIER_LIMIT];


pub struct ComponentModifier {
    pub(super) ptr: NonNull<()>,
    pub(super) f: Box<dyn FnOnce(NonNull<()>)>
}

impl std::hash::Hash for ComponentModifier {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.ptr.hash(state);
    }
}

impl PartialEq for ComponentModifier {
    fn eq(&self, other: &Self) -> bool {
        self.ptr == other.ptr
    }
}

impl PartialOrd for ComponentModifier {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        self.ptr.partial_cmp(&other.ptr)
    }
}

impl Eq for ComponentModifier { }

impl Ord for ComponentModifier {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.ptr.cmp(&other.ptr)
    }
}

impl ComponentModifier {
    pub fn get_ptr(&self) -> NonNull<()> {
        self.ptr
    }

    pub unsafe fn execute(self) {
        (self.f)(self.ptr);
    }
}


unsafe impl Send for ComponentModifier { }
unsafe impl Sync for ComponentModifier { }


enum MonoEnqueue {
    Modifier(ComponentModifier),
    Close
}


pub struct ComponentModifierStager {
    stages: Arc<Mutex<Vec<FxHashSet<ComponentModifier>>>>,
    mod_sender: SyncSender<MonoEnqueue>
}

impl ComponentModifierStager {
    pub fn new() -> Self {
        let (mod_sender, mod_recv) = sync_channel(32);
        let stages = Arc::new(Mutex::new(vec![FxHashSet::default()]));
        let stages_cloned = stages.clone();

        spawn(move || {
            loop {
                let Ok(x) = mod_recv.recv() else { return };
                let MonoEnqueue::Modifier(modifier) = x else { continue };

                let mut stages = stages_cloned.lock();
                unsafe {
                    stages.last_mut().unwrap_unchecked().insert(modifier);
                }

                'a: loop {
                    let Ok(x) = mod_recv.recv() else { return };
                    let MonoEnqueue::Modifier(modifier) = x else { break };
                    
                    for stage in stages.iter_mut() {
                        if !stage.contains(&modifier) {
                            stage.insert(modifier);
                            continue 'a
                        }
                    }
                    let mut stage = FxHashSet::with_capacity_and_hasher(
                        unsafe { 
                            stages.last().unwrap_unchecked().capacity()
                        },
                        Default::default()
                    );
                    stage.insert(modifier);
                    stages.push(stage);
                }
            }
        });

        Self {
            mod_sender,
            stages
        }
    }


    pub fn queue_modifier(&self, modifier: ComponentModifier) {
        unsafe {
            self.mod_sender.send(MonoEnqueue::Modifier(modifier)).unwrap_unchecked();
        }
    }

    pub unsafe fn execute(&mut self) {
        // println!("{}", self.stages.get_mut().iter().map(|x| x.len()).sum::<usize>());
        self.mod_sender.send(MonoEnqueue::Close).unwrap_unchecked();

        for stage in self.stages.lock().iter_mut() {
            stage
                .par_drain()
                .for_each(|x| x.execute());
        }
    }
}

#[derive(Default)]
struct MultiModificationStage {
    ptrs: FxHashSet<NonNull<()>>,
    modifiers: Vec<MultiComponentModifier>
}


unsafe impl Send for MultiModificationStage { }
unsafe impl Sync for MultiModificationStage { }


pub struct MultiComponentModifierStager {
    stages: Vec<MultiModificationStage>,
    queue: ConcurrentQueue<MultiComponentModifier>
}

impl Default for MultiComponentModifierStager {
    fn default() -> Self {
        Self { stages: Default::default(), queue: ConcurrentQueue::unbounded() }
    }
}


impl MultiComponentModifierStager {
    pub fn queue_modifier(&self, modifier: MultiComponentModifier) {
        assert!(self.queue.push(modifier).is_ok());
    }

    pub fn add_modifier(&mut self, modifier: MultiComponentModifier) {
        'a: for stage in &mut self.stages {
            for ptr in modifier.ptrs {
                let Some(ptr) = ptr else {
                    break
                };
                if stage.ptrs.contains(&ptr) {
                    continue 'a
                }
            }
            stage.ptrs.extend(modifier.ptrs.into_iter().map_while(|x| x));
            stage.modifiers.push(modifier);
            return
        }
        let mut stage = MultiModificationStage::default();
        stage.ptrs.extend(modifier.ptrs.into_iter().map_while(|x| x));
        stage.modifiers.push(modifier);
        self.stages.push(stage);
    }

    pub unsafe fn execute(&mut self) {
        while let Ok(modifier) = self.queue.pop() {
            self.add_modifier(modifier);
        }

        for mut stage in self.stages.drain(..) {
            stage
                .modifiers
                .par_drain(..)
                .for_each(|x| x.execute());
        }
    }
}


pub struct MultiComponentModifier {
    ptrs: PtrArray,
    f: Box<dyn FnOnce(PtrArray)>
}


unsafe impl Send for MultiComponentModifier { }
unsafe impl Sync for MultiComponentModifier { }


impl MultiComponentModifier {
    pub fn get_ptrs(&self) -> PtrArray {
        self.ptrs
    }
    
    pub unsafe fn execute(self) {
        (self.f)(self.ptrs)
    }
}


pub trait MultiQuery {
    type Arguments;

    fn queue_mut(self, f: impl FnOnce(Self::Arguments) + 'static);
}


impl<'a, C1, C2, S> MultiQuery for (Query<'a, C1, S>, Query<'a, C2, S>)
where
    C1: Component + 'a,
    C2: Component + 'a,
    S: ComponentContainer<'a, C1> + ComponentContainer<'a, C2> + ComponentModifierContainer
{
    type Arguments = (&'a mut C1, &'a mut C2);

    fn queue_mut(self, f: impl FnOnce(Self::Arguments) + 'static) {
        self
            .0
            .universe
            .queue_multi_component_modifier(
                MultiComponentModifier {
                    ptrs: [Some(NonNull::from(self.0.component).cast()), Some(NonNull::from(self.1.component).cast()), None, None],
                    f: Box::new(
                        |[ptr1, ptr2, _, _]| unsafe {
                            let mut ptr1 = ptr1.unwrap_unchecked().cast::<C1>();
                            let mut ptr2 = ptr2.unwrap_unchecked().cast::<C2>();
                            (f)((ptr1.as_mut(), ptr2.as_mut()))
                        }
                    )
                }
            )
    }
}