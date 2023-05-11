#![feature(exit_status_error)]

use std::{
    fs::File,
    io::{stderr, stdout, Write},
    process::Command,
    sync::mpsc::{sync_channel},
    time::Instant, f64::consts::PI,
};

use float_ord::FloatOrd;
use image::{codecs::gif::GifEncoder, Delay, Frame, ImageBuffer, Rgba};
use jaya_ecs::{
    component::{Component},
    extract::{
        Query,
    },
    rayon::{
        join, slice::ParallelSliceMut,
    },
    system::System,
    universe::{Universe},
};
use nalgebra::{Vector2, Rotation2};

const FPS: usize = 120;
const DELTA: f64 = 1.0 / FPS as f64;

const FRAME_WIDTH: u32 = 600;
const SPEED_BOUNDS: f64 = 0.025;

const MIN_MASS: f64 = 100.0;
const MAX_MASS: f64 = 10000.0;

const MIN_DISTANCE: f64 = 5.0;

const MIN_AGE: f64 = 0.75;
const MAX_AGE: f64 = 100.0;

const FRAME_COUNT: usize = (30.0 * FPS as f32) as usize;
const PARTICLE_COUNT: usize = 1500;

const FRAME_QUANTILE: usize = 5;

const SQUARE_PIXELS_PER_PARTICLE: usize = 300;
const SPAWN_RADIUS_SQUARED: f64 = (SQUARE_PIXELS_PER_PARTICLE * PARTICLE_COUNT) as f64 / PI;

#[derive(Debug, Clone, Copy)]
struct ParticleComponent {
    origin: Vector2<f64>,
    velocity: Vector2<f64>,
    mass: f64,
    age: f64,
}

impl ParticleComponent {
    fn rand() -> Self {
        Self {
            origin: Rotation2::new(fastrand::f64() * 2.0 * PI) * Vector2::new(
                SPAWN_RADIUS_SQUARED.sqrt() * fastrand::f64(),
                0.0,
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

impl Component for ParticleComponent {}


fn attraction([p1, p2]: [Query<ParticleComponent>; 2]) {
    const GRAVITATIONAL_CONST: f64 = 0.000000001;

    let mut travel = p1.origin - p2.origin;

    let distance = travel.magnitude().max(MIN_DISTANCE);
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

fn ageing(p1: Query<ParticleComponent>, universe: &Universe) {
    const FORCE_FACTOR: f64 = 2.0;
    const AGE_FACTOR: f64 = 0.75;
    if p1.age > DELTA {
        p1.queue_mut(|p1| p1.age -= DELTA);
        return;
    }

    let explode = |p2: Query<ParticleComponent>| {
        if p1 == p2 {
            return;
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

#[inline(always)]
fn mix_pixel(src: &mut Rgba<u8>, color: Rgba<u8>) {
    src[0] = src[0] / 2 + color[0] / 2;
    src[1] = src[1] / 2 + color[1] / 2;
    src[2] = src[2] / 2 + color[2] / 2;
}

#[inline(always)]
fn draw_star(imgbuf: &mut ImageBuffer<Rgba<u8>, Vec<u8>>, x: u32, y: u32, color: Rgba<u8>) {
    imgbuf
        .get_pixel_mut_checked(x, y)
        .map(|p| mix_pixel(p, color));
    
    let side_color = Rgba([
        (color[0] as f64 * 0.7) as u8,
        (color[1] as f64 * 0.7) as u8,
        (color[2] as f64 * 0.7) as u8,
        255
    ]);
    let corner_color = Rgba([
        (color[0] as f64 * 0.5) as u8,
        (color[1] as f64 * 0.5) as u8,
        (color[2] as f64 * 0.5) as u8,
        255
    ]);
    
    imgbuf
        .get_pixel_mut_checked(x + 1, y)
        .map(|p| mix_pixel(p, side_color));
    imgbuf
        .get_pixel_mut_checked(x, y + 1)
        .map(|p| mix_pixel(p, side_color));
    imgbuf
        .get_pixel_mut_checked(x + 1, y + 1)
        .map(|p| mix_pixel(p, corner_color)); 
    if x > 0 {
        imgbuf
            .get_pixel_mut_checked(x - 1, y)
            .map(|p| mix_pixel(p, side_color));
        imgbuf
            .get_pixel_mut_checked(x - 1, y + 1)
            .map(|p| mix_pixel(p, corner_color));
        if y > 0 {
            imgbuf
                .get_pixel_mut_checked(x - 1, y - 1)
                .map(|p| mix_pixel(p, corner_color)); 
        }
    }
    if y > 0 {
        imgbuf
            .get_pixel_mut_checked(x, y - 1)
            .map(|p| mix_pixel(p, side_color)); 
        imgbuf
            .get_pixel_mut_checked(x + 1, y - 1)
            .map(|p| mix_pixel(p, corner_color)); 
    }
}

fn main() {
    let (sender, receiver) = sync_channel::<Vec<ParticleComponent>>(FRAME_COUNT);

    let mut particles = Universe::<()>::default();
    
    for _i in 0..PARTICLE_COUNT {
        particles.add_entity((ParticleComponent::rand(),));
    }

    particles.process_queues();
    
    let start = Instant::now();
    println!("Starting");

    join(
        || {
            let mut gif = GifEncoder::new(File::create("simulation.gif").unwrap());

            receiver.into_iter().enumerate().for_each(|(i, frame_data)| {
                let mut imgbuf = image::ImageBuffer::from_pixel(
                    FRAME_WIDTH,
                    FRAME_WIDTH,
                    Rgba([0u8, 0u8, 0u8, 255u8]),
                );

                if frame_data.is_empty() {
                    let frame = Frame::from_parts(
                        imgbuf,
                        0,
                        0,
                        Delay::from_numer_denom_ms((DELTA * 1000.0) as u32, 1000),
                    );
                    gif.encode_frame(frame).unwrap();
                    return;
                }

                let particle_count = frame_data.len();
                let max_vel = frame_data.iter().map(|p| FloatOrd(p.velocity.magnitude())).max().unwrap().0;
                let mut origins: Vec<_> = frame_data.iter().map(|p| p.origin).collect();
                let center = origins.iter().sum::<Vector2<f64>>().scale(1.0 / particle_count as f64);

                origins.par_sort_unstable_by_key(|origin| FloatOrd(origin.x - center.x));
                let lower_x = origins.get(origins.len() / FRAME_QUANTILE).unwrap().x;
                let upper_x = origins.get(origins.len() * (FRAME_QUANTILE - 1) / FRAME_QUANTILE).unwrap().x;

                origins.par_sort_unstable_by_key(|origin| FloatOrd(origin.y - center.y));
                let lower_y = origins.get(origins.len() / FRAME_QUANTILE).unwrap().y;
                let upper_y = origins.get(origins.len() * (FRAME_QUANTILE - 1) / FRAME_QUANTILE).unwrap().y;

                let true_frame_height = upper_y - lower_y;
                let true_frame_width = upper_x - lower_x;

                for p in frame_data {
                    let r = (255.0 * p.velocity.magnitude() / max_vel) as u8;
                    let b = (255.0 * p.mass / MAX_MASS) as u8;
                    let g = r / 2 + b / 2;

                    // let luma = (r as f32 + g as f32 + b as f32) / 3.0 / 255.0;
                    // let scale = ((1.0 - luma) * 0.5 + luma) / luma;
                    // let r = (r as f32 * scale) as u8;
                    // let g = (g as f32 * scale) as u8;
                    // let b = (b as f32 * scale) as u8;

                    let x = (p.origin.x - lower_x) / true_frame_width * FRAME_WIDTH as f64;
                    let y = (p.origin.y - lower_y) / true_frame_height * FRAME_WIDTH as f64;

                    if x >= 0.0 && y >= 0.0 {
                        draw_star(&mut imgbuf, x as u32, y as u32, Rgba([r, g, b, 255]));
                    }
                }

                let frame = Frame::from_parts(
                    imgbuf,
                    0,
                    0,
                    Delay::from_numer_denom_ms((DELTA * 1000.0) as u32, 1000),
                );
                gif.encode_frame(frame).unwrap();

                if i == 0 {
                    return;
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
            });
        },
        || {
            for _i in 0..FRAME_COUNT {
                join(
                    || {
                        (attraction, ageing).run_once(&particles);
                    },
                    || {
                        let frame_data: Vec<_> = particles
                            .iter_components_collect(|_, x: &ParticleComponent| *x);
                        sender.send(frame_data).unwrap();
                    }
                );
                
                particles.process_queues();
            }

            drop(sender);
            println!("Simulation done in {}s", start.elapsed().as_secs_f32());
        },
    );

    println!("Video done in {}s", start.elapsed().as_secs_f32());
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
