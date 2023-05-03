#![feature(exit_status_error)]

use std::{fs::File, sync::mpsc::channel, time::Instant, process::{Command}, io::{stderr, stdout, Write}};

use concurrent_queue::ConcurrentQueue;
use float_ord::FloatOrd;
use image::{codecs::gif::GifEncoder, Delay, Frame, Rgba, ImageBuffer};
use itertools::Itertools;
use jaya_ecs::{
    rayon::{
        prelude::{IntoParallelRefIterator, ParallelIterator, ParallelBridge},
        join,
    }, component::{Component, IntoComponentIterator}, entity::EntityId, extract::{ComponentModifierStager, MultiComponentModifierStager, ComponentModifier, MultiComponentModifier, Query}, system::System, universe::Universe,
};
use nalgebra::{Vector2, Matrix, Const, ArrayStorage};
use rustc_hash::FxHashMap;

const DELTA: f64 = 1.0 / 120.0;

const SPACE_BOUNDS: f64 = 200.0;
const SPEED_BOUNDS: f64 = 0.025;

const MIN_MASS: f64 = 100.0;
const MAX_MASS: f64 = 10000.0;

const MIN_DISTANCE: f64 = 5.0;

const MIN_AGE: f64 = 0.75;
const MAX_AGE: f64 = 100.0;

const FRAME_COUNT: usize = 100;
const PARTICLE_COUNT: usize = 1000;

#[derive(Debug)]
struct ParticleComponent {
    origin: Vector2<f64>,
    velocity: Vector2<f64>,
    mass: f64,
    age: f64
}

impl ParticleComponent {
    fn rand() -> Self {
        Self {
            origin: Vector2::new(
                (fastrand::f64() - 0.5) * 2.0 * SPACE_BOUNDS,
                (fastrand::f64() - 0.5) * 2.0 * SPACE_BOUNDS,
            ),
            velocity: Vector2::new(
                (fastrand::f64() - 0.5) * 2.0 * SPEED_BOUNDS,
                (fastrand::f64() - 0.5) * 2.0 * SPEED_BOUNDS,
            ),
            mass: MIN_MASS + fastrand::f64() * (MAX_MASS - MIN_MASS),
            age: MIN_AGE + fastrand::f64() * (MAX_AGE - MIN_AGE),
        }
    }
}

impl Component for ParticleComponent { }

struct Entities {
    entities: FxHashMap<EntityId, ParticleComponent>,
    add_queue: ConcurrentQueue<(EntityId, ParticleComponent)>,
    remove_queue: ConcurrentQueue<EntityId>,
    mod_queue: ComponentModifierStager,
    multi_mod_queue: MultiComponentModifierStager
}

impl IntoComponentIterator<ParticleComponent> for Entities {
    fn iter_component_combinations<'a, F, const N: usize>(&'a self, f: F)
    where
        F: Fn([(EntityId, &'a ParticleComponent); N]) + Sync {
        self
            .entities
            .iter()
            .combinations(N)
            .par_bridge()
            .for_each(|vec| {
                let arr: [(&EntityId, &ParticleComponent); N] = vec.try_into().unwrap();
                let arr = arr.map(|x| (*x.0, x.1));
                (f)(arr)
            })
    }

    fn iter_components<'a, F>(&'a self, f: F)
    where
        F: Fn(EntityId, &'a ParticleComponent) + Sync
    {
            self.entities.par_iter().for_each(|x| (f)(*x.0, x.1));
    }
}

impl Universe for Entities {
    fn queue_component_modifier(&self, modifier: ComponentModifier) {
        self.mod_queue.queue_modifier(modifier);
    }

    fn queue_multi_component_modifier(&self, modifier: MultiComponentModifier) {
        self.multi_mod_queue.queue_modifier(modifier);
    }

    fn queue_remove_entity(&self, id: EntityId) {
        self.remove_queue.push(id).unwrap();
    }
}

impl Entities {
    pub fn process_queues(&mut self) {
        unsafe {
            self.multi_mod_queue.execute();
            self.mod_queue.execute();
        }
        for (id, component) in self.add_queue.try_iter() {
            self.entities.insert(id, component);
        }
        for id in self.remove_queue.try_iter() {
            self.entities.remove(&id);
        }
    }
}


fn attraction([p1, p2]: [Query<ParticleComponent>; 2]) {
    const GRAVITATIONAL_CONST: f64 = 0.000000001;

    let mut travel = p1.origin - p2.origin;
    let distance = travel.magnitude().max(MIN_DISTANCE);
    let force = GRAVITATIONAL_CONST * p1.mass * p2.mass / distance;
    travel = travel.normalize();

    let p1_delta = - force * travel * DELTA;
    let p2_delta = force * travel * DELTA;

    // (p1, p2)
    //     .queue_mut(move |(p1, p2)| {
    //         p1.origin += p1.velocity * DELTA;
    //         p1.origin += p1_delta;
            
    //         p2.origin += p2.velocity * DELTA;
    //         p2.origin += p2_delta;
    //     });
    p1.queue_mut(move |x| {
        x.origin += x.velocity * DELTA;
        x.velocity += p1_delta;
    });
    p2.queue_mut(move |x| {
        x.origin += x.velocity * DELTA;
        x.velocity += p2_delta;
    });
}


fn ageing(p1: Query<ParticleComponent>, universe: &Entities) {
    const FORCE_FACTOR: f64 = 1.0;
    const AGE_FACTOR: f64 = 0.5;
    if p1.age > DELTA {
        p1.queue_mut(|p1| p1.age -= DELTA);
        return
    }

    let explode = |p2: Query<ParticleComponent>| {
        if p1 == p2 {
            return
        }
        let mut travel = p2.origin - p1.origin;
        let distance = travel.magnitude().max(MIN_DISTANCE);
        travel = travel.normalize();
        let impulse = FORCE_FACTOR * p1.mass / p2.mass / distance;
        let vel_delta = impulse * travel;
        let age_factor = impulse * AGE_FACTOR;

        p2.queue_mut(move |p2| {
            p2.velocity += vel_delta;
            p2.age -= age_factor;
        });
    };
    universe.queue_remove_entity(p1.get_id());
    explode.run_once(universe);
}


fn main() {
    let (sender, receiver) = channel();

    let mut particles: Entities = Entities {
        entities: Default::default(),
        add_queue: ConcurrentQueue::unbounded(),
        remove_queue: ConcurrentQueue::unbounded(),
        mod_queue: ComponentModifierStager::new(),
        multi_mod_queue: MultiComponentModifierStager::new()
    };

    for _i in 0..PARTICLE_COUNT {
        particles.entities.insert(
            EntityId::rand(),
            ParticleComponent::rand(),
        );
    }

    let max_mass = particles.entities.iter().map(|x| FloatOrd(x.1.mass)).max().unwrap().0;
    let start = Instant::now();
    
    join(
        || {
            let mut gif = GifEncoder::new(File::create("simulation.gif").unwrap());
            let mut current_index: usize = 0;
            let mut out_of_order = vec![];

            let mut encode_frame = |
                    frame_data: Vec<(Matrix<f64, Const<2>, Const<1>, ArrayStorage<f64, 2, 1>>, f64, f64)>
                | {
                    if frame_data.is_empty() {
                        return
                    }
                    let max_vel = frame_data.iter().map(|x| FloatOrd(x.1)).max().unwrap().0;

                    let mut imgbuf = image::ImageBuffer::from_pixel(SPACE_BOUNDS as u32 * 2, SPACE_BOUNDS as u32 * 2, Rgba([0u8, 0u8, 0u8, 255u8]));

                    for (origin, speed, mass) in frame_data {
                        let mut r = (255.0 * speed / max_vel) as u8;
                        let mut b = (255.0 * mass / max_mass) as u8;
                        let mut g = (r + b) / 2;

                        let luma = (r as f32 + g as f32 + b as f32) / 3.0 / 255.0;
                        let scale = ((1.0 - luma) * 0.5 + luma) / luma;
                        r = (r as f32 * scale) as u8;
                        g = (g as f32 * scale) as u8;
                        b = (b as f32 * scale) as u8;

                        let mix = |x: &mut Rgba<u8>| {
                            *x = Rgba([
                                ((x.0[0] as f32 + r as f32) / 2f32) as u8,
                                ((x.0[1] as f32 + g as f32) / 2f32) as u8,
                                ((x.0[2] as f32 + b as f32) / 2f32) as u8,
                                255u8
                            ])
                        };

                        fn mod_pixel(imgbuf: &mut ImageBuffer<Rgba<u8>, Vec<u8>>, mut x: f64, mut y: f64, f: impl Fn(&mut Rgba<u8>)) {
                            x += SPACE_BOUNDS;
                            y += SPACE_BOUNDS;
                            if x < 0.0 || y < 0.0 {
                                // Do nothing
                            } else {
                                imgbuf.get_pixel_mut_checked(x as u32, y as u32).map(f);
                            }
                        }

                        mod_pixel(&mut imgbuf, origin.x, origin.y, mix);
                        
                        r = (r as f32 * 0.7) as u8;
                        g = (g as f32 * 0.7) as u8;
                        b = (b as f32 * 0.7) as u8;
                        let mix = |x: &mut Rgba<u8>| {
                            *x = Rgba([
                                ((x.0[0] as f32 + r as f32) / 2f32) as u8,
                                ((x.0[1] as f32 + g as f32) / 2f32) as u8,
                                ((x.0[2] as f32 + b as f32) / 2f32) as u8,
                                255u8
                            ])
                        };

                        mod_pixel(&mut imgbuf, origin.x + 1.0, origin.y, mix);
                        mod_pixel(&mut imgbuf, origin.x - 1.0, origin.y, mix);
                        mod_pixel(&mut imgbuf, origin.x, origin.y + 1.0, mix);
                        mod_pixel(&mut imgbuf, origin.x, origin.y - 1.0, mix);

                        r = (r as f32 * 0.5) as u8;
                        g = (g as f32 * 0.5) as u8;
                        b = (b as f32 * 0.5) as u8;
                        let mix = |x: &mut Rgba<u8>| {
                            *x = Rgba([
                                ((x.0[0] as f32 + r as f32) / 2f32) as u8,
                                ((x.0[1] as f32 + g as f32) / 2f32) as u8,
                                ((x.0[2] as f32 + b as f32) / 2f32) as u8,
                                255u8
                            ])
                        };
                        mod_pixel(&mut imgbuf, origin.x + 1.0, origin.y + 1.0, mix);
                        mod_pixel(&mut imgbuf, origin.x - 1.0, origin.y + 1.0, mix);
                        mod_pixel(&mut imgbuf, origin.x + 1.0, origin.y - 1.0, mix);
                        mod_pixel(&mut imgbuf, origin.x - 1.0, origin.y - 1.0, mix);
                    }

                    let frame = Frame::from_parts(
                        imgbuf,
                        0,
                        0,
                        Delay::from_numer_denom_ms((DELTA * 1000.0) as u32, 1000),
                    );
                    gif.encode_frame(frame).unwrap();
            };

            receiver.into_iter().for_each(|(i, frame_data)| {
                if i == current_index {
                    (encode_frame)(frame_data);
                    current_index += 1;

                    if i == 0 {
                        println!("Starting");
                        return
                    }

                    if i % (FRAME_COUNT as f32 * 0.1) as usize == 0 {
                        let elapsed = start.elapsed().as_secs_f32();
                        println!(
                            "{}%    {}s    {}s remaining",
                            (i as f32 / FRAME_COUNT as f32 * 100.0) as u8,
                            elapsed,
                            elapsed * (FRAME_COUNT as f32 / i as f32 - 1.0)
                        );
                    }
                } else {
                    eprintln!("Out of order");
                    out_of_order.push((i, frame_data));
                    for (i, _) in &out_of_order {
                        if *i == current_index {
                            (encode_frame)(out_of_order.remove(*i).1);
                            current_index += 1;
                            break;
                        }
                    }
                }
            });
        },
        || {
            for i in 0..FRAME_COUNT {
                (attraction, ageing).run_once(&particles);
                particles.process_queues();

                let frame_data = particles.entities.iter().map(|x| (x.1.origin, x.1.velocity.magnitude(), x.1.mass)).collect_vec();
                sender.send((i, frame_data)).unwrap();
            }

            drop(sender);
        }
    );

    println!("Done in {}s", start.elapsed().as_secs_f32());
    let output = Command::new("ffmpeg")
        .args("-i simulation.gif -movflags faststart -pix_fmt yuv420p -vf scale=800:800:flags=neighbor,setpts=PTS/12,fps=120 -b:v 8M simulation.mp4 -y".split_ascii_whitespace())
        .output()
        .unwrap();

    if output.status.exit_ok().is_ok() {
        println!("Video conversion successful");
    } else {
        println!("ffmpeg status: {}", output.status);
        stdout().write_all(&output.stdout).unwrap();
        stderr().write_all(&output.stderr).unwrap();
    }
}
