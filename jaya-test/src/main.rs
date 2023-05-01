use std::{fs::File, sync::mpsc::channel, time::Instant};

use concurrent_queue::ConcurrentQueue;
use image::{codecs::gif::GifEncoder, Delay, Frame, Rgba};
use jaya_ecs::{
    rayon::{
        prelude::{IntoParallelRefIterator, ParallelIterator},
        spawn, iter::Map, collections::hash_map::Iter,
    }, component::{Component, ComponentModifierContainer, ComponentContainer}, entity::EntityId, extract::{ComponentModifierStager, MultiComponentModifierStager, ComponentModifier, MultiComponentModifier}, system::System, universe::Universe,
};
use nalgebra::Vector2;
use rand::{rngs::ThreadRng, thread_rng, Rng};
use rustc_hash::FxHashMap;

const GRAVITATIONAL_CONST: f64 = 0.00000001;
const DELTA: f64 = 0.008;
const SPACE_BOUNDS: f64 = 100.0;
const SPEED_BOUNDS: f64 = 0.005;
const MIN_MASS: f64 = 100.0;
const MAX_MASS: f64 = 10000.0;
const MIN_DISTANCE: f64 = 5.0;
const FRAME_COUNT: usize = 500;

struct ParticleComponent {
    origin: Vector2<f64>,
    velocity: Vector2<f64>,
    mass: f64,
}

impl ParticleComponent {
    fn rand(rng: &mut ThreadRng) -> Self {
        Self {
            origin: Vector2::new(
                rng.gen_range(-SPACE_BOUNDS..=SPACE_BOUNDS),
                rng.gen_range(-SPACE_BOUNDS..=SPACE_BOUNDS),
            ),
            velocity: Vector2::new(
                rng.gen_range(-SPEED_BOUNDS..=SPEED_BOUNDS),
                rng.gen_range(-SPEED_BOUNDS..=SPEED_BOUNDS),
            ),
            mass: rng.gen_range(MIN_MASS..MAX_MASS),
        }
    }
}

impl Component for ParticleComponent {}

struct Entities {
    entities: FxHashMap<EntityId, ParticleComponent>,
    add_queue: ConcurrentQueue<(EntityId, ParticleComponent)>,
    remove_queue: ConcurrentQueue<EntityId>,
    mod_queue: ComponentModifierStager,
    multi_mod_queue: MultiComponentModifierStager
}

impl<'a> ComponentContainer<'a, ParticleComponent> for Entities {
    type Iterator = Map<Iter<'a, EntityId, ParticleComponent>, fn((&'a EntityId, &'a ParticleComponent)) -> (EntityId, &'a ParticleComponent)>;

    fn queue_add_component(&self, id: EntityId, component: ParticleComponent) {
        assert!(self.add_queue.push((id, component)).is_ok());
    }

    fn queue_remove_component(&self, id: EntityId) {
        assert!(self.remove_queue.push(id).is_ok());
    }

    fn iter_components(&'a self) -> Self::Iterator {
        self.entities.par_iter().map(|x| (*x.0, x.1))
    }
}

impl ComponentModifierContainer for Entities {
    fn queue_component_modifier(&self, modifier: ComponentModifier) {
        self.mod_queue.queue_modifier(modifier);
    }

    fn queue_multi_component_modifier(&self, modifier: MultiComponentModifier) {
        self.multi_mod_queue.queue_modifier(modifier);
    }
}

impl Entities {
    pub fn process_queues(&mut self) {
        // println!("modifying");
        unsafe {
            self.multi_mod_queue.execute();
            self.mod_queue.execute();
        }
        // println!("done");
        for (id, component) in self.add_queue.try_iter() {
            self.entities.insert(id, component);
        }
        for id in self.remove_queue.try_iter() {
            self.entities.remove(&id);
        }
    }
}


impl Universe for Entities { }


type Query<'a, T> = jaya_ecs::extract::Query<'a, T, Entities>;


fn attraction(p1: Query<ParticleComponent>, p2: Query<ParticleComponent>) {
    if p1 == p2 {
        return
    }

    let mut travel = p1.origin - p2.origin;
    let distance = travel.magnitude_squared().max(MIN_DISTANCE * MIN_DISTANCE);
    let force = GRAVITATIONAL_CONST * p1.mass * p2.mass / distance;
    travel = travel.normalize();

    let p1_delta = - force * travel * DELTA;
    let p2_delta = force * travel * DELTA;

    p1.queue_mut(move |x| {
        x.origin += x.velocity * DELTA;
        x.velocity += p1_delta;
    });
    p2.queue_mut(move |x| {
        x.origin += x.velocity * DELTA;
        x.velocity += p2_delta;
    });
}


fn main() {
    println!("Initializing");
    let mut start = Instant::now();
    let mut particles: Entities = Entities {
        entities: Default::default(),
        add_queue: ConcurrentQueue::unbounded(),
        remove_queue: ConcurrentQueue::unbounded(),
        mod_queue: ComponentModifierStager::new(),
        multi_mod_queue: Default::default()
    };

    let mut rng = thread_rng();
    for _i in 0..200 {
        particles.entities.insert(
            EntityId::rand_with_rng(&mut rng),
            ParticleComponent::rand(&mut rng),
        );
    }
    drop(rng);

    let (sender, receiver) = channel();

    spawn(|| {
        let mut gif = GifEncoder::new(File::create("simulation.gif").unwrap());
        let mut current_index: usize = 0;
        let mut out_of_order = vec![];

        receiver.into_iter().for_each(|(i, img)| {
            if i == current_index {
                gif.encode_frame(img).unwrap();
                current_index += 1;

                if i % (FRAME_COUNT as f32 * 0.1) as usize == 0 {
                    println!("{}%", i as f32 / FRAME_COUNT as f32 * 100.0);
                }
            } else {
                println!("Out of order");
                out_of_order.push((i, img));
                for (i, _) in &out_of_order {
                    if *i == current_index {
                        gif.encode_frame(out_of_order.remove(*i).1).unwrap();
                        current_index += 1;
                        break;
                    }
                }
            }
        });
    });

    println!("Initialization complete {}", start.elapsed().as_secs_f32());
    start = Instant::now();
    for i in 0..FRAME_COUNT {
        attraction.run_once(&particles);
        particles.process_queues();

        let frame_data: Vec<_> = particles.entities.par_iter().map(|x| x.1.origin).collect();

        let sender = sender.clone();

        spawn(move || {
            let mut imgbuf =
                image::ImageBuffer::new(SPACE_BOUNDS as u32 * 2, SPACE_BOUNDS as u32 * 2);

            for origin in frame_data {
                if origin.x.abs() > SPACE_BOUNDS || origin.y.abs() > SPACE_BOUNDS {
                    continue;
                }
                *imgbuf.get_pixel_mut((origin.x + 100.0) as u32, (origin.y + 100.0) as u32) =
                    Rgba([255u8; 4]);
            }

            let frame = Frame::from_parts(
                imgbuf,
                0,
                0,
                Delay::from_numer_denom_ms((DELTA * 1000.0) as u32, 1000),
            );
            sender.send((i, frame)).unwrap();
        });
    }

    drop(sender);

    println!("Done in {}", start.elapsed().as_secs_f32());
}
