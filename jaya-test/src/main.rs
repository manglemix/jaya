//! An example usage of `jaya-ecs`
//! 
//! Particles are simulated with Newtonian Gravity, and each particle has a random lifespan.
//! When a particle's lifespan expires, it has a 50% chance of exploding, which can reduce the lifespan of nearby particles,
//! or split into two equal mass particles.
//! 
//! A preview of the render is displayed in a window, while a video is compiled concurrently using FFMPEG. Thus, FFMPEG must be
//! installed separately and accessible from command line.
#![feature(exit_status_error)]

use std::{
    io::{stderr, Write, BufWriter},
    process::{Command, Stdio, exit},
    sync::mpsc::{sync_channel},
    time::Instant, f64::consts::PI,
};

use flo_draw::{create_drawing_window, canvas::{GraphicsPrimitives, Color, GraphicsContext}, with_2d_graphics, initialize_offscreen_rendering, render_canvas_offscreen};
use float_ord::FloatOrd;
use futures::{executor::block_on, stream};
use jaya_ecs::{
    component::{Component},
    extract::{
        Query,
    },
    rayon::{
        join, slice::ParallelSliceMut, prelude::{IntoParallelIterator, ParallelIterator},
    },
    system::System,
    universe::{Universe},
};
use nalgebra::{Vector2, Rotation2};

// Number of pixels in a single frame in the output video
const FRAME_WIDTH: usize = 800;
// How long the final video should be
const VIDEO_DURATION: f32 = 25.0;
// The framerate of the final video
const FPS: usize = 120;

// The maximum speed in the x and y direction when randomly generating velocities
const SPEED_BOUNDS: f64 = 10.0;
// Mass bounds for particles
const MIN_MASS: f64 = 100.0;
const MAX_MASS: f64 = 10000.0;
// How long a particle can live for before it explodes or splits
const MIN_LIFESPAN: f64 = 0.75;
const MAX_LIFESPAN: f64 = 100.0;
// Affects how large a particle can be based on its mass
const STAR_RADIUS_FACTOR: f64 = 0.1;

// The number of particles at the beginning of the simulation
const PARTICLE_COUNT: usize = 1500;
// The number of pixels per particle at the beginning of the simulation
// Essentially, bigger numbers mean more spread out
const PIXELS_PER_PARTICLE: usize = 600;

// The minimum distance between particles to avoid divide-by-zero errors
const MIN_DISTANCE: f64 = 5.0;

// To draw the red cross at the origin
const CENTER_RECT_HALF_WIDTH: f32 = 1.0;
const CENTER_RECT_HALF_HEIGHT: f32 = 4.0;
const CENTER_CROSS_BRIGHTNESS: f32 = 0.3;


// The speed in pixels that the camera's bounds can move to fit the universe
const CAMERA_MOVE_SPEED: f64 = 5.0;
// Higher numbers make the camera view more of the universe at once
const FRAME_QUANTILE: usize = 6;

// These constants are calculated automatically
const SPAWN_RADIUS_SQUARED: f64 = (PIXELS_PER_PARTICLE * PARTICLE_COUNT) as f64 / PI;
const FRAME_COUNT: usize = (VIDEO_DURATION * FPS as f32) as usize;
const DELTA: f64 = 1.0 / FPS as f64;

#[derive(Debug, Clone, Copy)]
struct ParticleComponent {
    origin: Vector2<f64>,
    velocity: Vector2<f64>,
    mass: f64,
    lifespan: f64,
}

impl ParticleComponent {
    fn rand() -> Self {
        let bounds = SPAWN_RADIUS_SQUARED.sqrt();
        let mut origin = Vector2::new(
            (fastrand::f64() - 0.5) * 2.0 * bounds,
            (fastrand::f64() - 0.5) * 2.0 * bounds,
        );
        while origin.magnitude() > bounds {
            origin = Vector2::new(
                (fastrand::f64() - 0.5) * 2.0 * bounds,
                (fastrand::f64() - 0.5) * 2.0 * bounds,
            );
        }
        Self {
            origin,
            velocity: Vector2::new(
                (fastrand::f64() - 0.5) * 2.0 * SPEED_BOUNDS,
                (fastrand::f64() - 0.5) * 2.0 * SPEED_BOUNDS,
            ),
            mass: MIN_MASS + fastrand::f64() * (MAX_MASS - MIN_MASS),
            lifespan: MIN_LIFESPAN + fastrand::f64() * (MAX_LIFESPAN - MIN_LIFESPAN),
        }
    }
}

impl Component for ParticleComponent {}


fn attraction([p1, p2]: [Query<ParticleComponent>; 2]) {
    const GRAVITATIONAL_CONST: f64 = 0.0000001;

    let mut travel = p1.origin - p2.origin;

    let distance = travel.magnitude().max(MIN_DISTANCE);
    let force = GRAVITATIONAL_CONST * p1.mass * p2.mass / distance;
    travel = travel.normalize();

    let p1_delta = - force * travel * DELTA;
    let p2_delta = force * travel * DELTA;

    p1.queue_mut(move |x| {
        x.velocity += p1_delta;
    });
    p2.queue_mut(move |x| {
        x.velocity += p2_delta;
    });
}

fn ageing(p1: Query<ParticleComponent>, universe: &Universe) {
    const FORCE_FACTOR: f64 = 5.0;
    const AGE_FACTOR: f64 = 2.0;
    if p1.lifespan > DELTA {
        p1.queue_mut(|p1| {
            p1.origin += p1.velocity * DELTA;
            p1.lifespan -= DELTA;
        });
        return;
    }

    if fastrand::bool() {
        let extra_vel = Rotation2::new(PI / 2.0) * p1.velocity;
        p1.queue_mut(move |p1| {
            p1.lifespan = MIN_LIFESPAN + fastrand::f64() * (MAX_LIFESPAN - MIN_LIFESPAN);
            p1.velocity += extra_vel;
            p1.origin += p1.velocity * DELTA;
        });
        let velocity = p1.velocity - extra_vel;
        universe.add_entity((ParticleComponent {
            origin: p1.origin + velocity * DELTA,
            velocity,
            mass: p1.mass,
            lifespan: MIN_LIFESPAN + fastrand::f64() * (MAX_LIFESPAN - MIN_LIFESPAN)
        },));
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
            p2.lifespan -= age_factor;
        });
    };
    universe.queue_remove_entity(p1.get_id());
    explode.run_once(universe);
}


enum FrameInfo {
    Particle(ParticleComponent),
    EndFrame
}


fn main() -> Result<(), std::io::Error> {
    let mut ffmpeg = Command::new("ffmpeg")
        .args(
            format!(
                "-y -f rawvideo -pix_fmt rgba -s {FRAME_WIDTH}x{FRAME_WIDTH} -r {FPS} -i - -c:v libx264 -profile:v baseline -level:v 3 -b:v 12M -vf format=yuv420p -an simulation.mp4"
            ).split_ascii_whitespace()
        )
        .stdin(Stdio::piped())
        .stderr(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()?;
    let mut ffmpeg_stdin = BufWriter::new(ffmpeg.stdin.take().unwrap());
    let mut ffmpeg_stderr = ffmpeg.stderr.take().unwrap();

    let (frame_sender, frame_receiver) = sync_channel::<FrameInfo>(FRAME_COUNT * PARTICLE_COUNT);
        // println!("{}", FRAME_COUNT * PARTICLE_COUNT);

    let mut particles = Universe::<()>::default();
    
    (0..PARTICLE_COUNT)
        .into_par_iter()
        .for_each(|_| {
            particles.add_entity((ParticleComponent::rand(),));
        });

    particles.process_queues();

    with_2d_graphics(move || {
        let ui = create_drawing_window("Preview");

        println!("Starting");

        join(
            move || {
                let start = Instant::now();
        
                for _i in 0..FRAME_COUNT {
                    (
                        attraction,
                        ageing,
                        |p1: Query<ParticleComponent>| {
                            frame_sender.send(FrameInfo::Particle(*p1)).unwrap();
                        }
                    ).run_once(&particles);
                    frame_sender.send(FrameInfo::EndFrame).unwrap();
                    
                    particles.process_queues();
                }
    
                println!("Simulation done in {}s", start.elapsed().as_secs_f32());
            },
            move || {
                let start = Instant::now();
        
                let mut canvas = initialize_offscreen_rendering().unwrap();

                macro_rules! send_to_ffmpeg {
                    ($frame: expr) => {
                        if let Err(e) = ffmpeg_stdin.write_all(&$frame) {
                            eprintln!("{e}");
                            std::io::copy(&mut ffmpeg_stderr, &mut stderr().lock()).expect("stderr copy to work");
                            exit(1);
                        }
                    };
                }

                let mut last_upper_x = 0.0;
                let mut last_upper_y = 0.0;
                let mut last_lower_x = 0.0;
                let mut last_lower_y = 0.0;

                for i in 0..FRAME_COUNT {
                    let mut drawing = vec![];
                    drawing.clear_canvas(Color::Rgba(0.0, 0.0, 0.0, 1.0));
                    drawing.canvas_height(FRAME_WIDTH as f32);
                    drawing.center_region(0.0, 0.0, FRAME_WIDTH as f32, FRAME_WIDTH as f32);

                    let FrameInfo::Particle(first_p) = frame_receiver.recv().unwrap() else {
                        ui.draw(|gc| gc.extend(drawing.clone()));
                        let frame = block_on(render_canvas_offscreen(&mut canvas, FRAME_WIDTH, FRAME_WIDTH, 1.0, stream::iter(drawing)));
                        send_to_ffmpeg!(frame);
                        continue;
                    };

                    let mut frame_data = vec![];
                    let mut max_vel = first_p.velocity.magnitude();
                    let mut center = first_p.origin;
                    frame_data.push((first_p.origin, max_vel, first_p.mass));

                    while let FrameInfo::Particle(p) = frame_receiver.recv().unwrap() {
                        let p_vel = p.velocity.magnitude();
                        if p_vel > max_vel {
                            max_vel = p_vel;
                        }
                        center += p.origin;
                        frame_data.push((p.origin, p_vel, p.mass));
                    }

                    center = center.scale(1.0 / frame_data.len() as f64);

                    frame_data.par_sort_unstable_by_key(|(origin, _, _)| FloatOrd(origin.x - center.x));
                    let mut lower_x = frame_data.get(frame_data.len() / FRAME_QUANTILE).unwrap().0.x;
                    let mut upper_x = frame_data.get(frame_data.len() * (FRAME_QUANTILE - 1) / FRAME_QUANTILE).unwrap().0.x;
    
                    frame_data.par_sort_unstable_by_key(|(origin, _, _)| FloatOrd(origin.y - center.y));
                    let mut lower_y = frame_data.get(frame_data.len() / FRAME_QUANTILE).unwrap().0.y;
                    let mut upper_y = frame_data.get(frame_data.len() * (FRAME_QUANTILE - 1) / FRAME_QUANTILE).unwrap().0.y;

                    if i > 0 {
                        const CAMERA_MOVE_STEP: f64 = CAMERA_MOVE_SPEED * DELTA;
                        if (lower_x - last_lower_x).abs() > CAMERA_MOVE_STEP {
                            lower_x = last_lower_x + (lower_x - last_lower_x) / (lower_x - last_lower_x).abs() * CAMERA_MOVE_STEP;
                        }
                        if (lower_y - last_lower_y).abs() > CAMERA_MOVE_STEP {
                            lower_y = last_lower_y + (lower_y - last_lower_y) / (lower_y - last_lower_y).abs() * CAMERA_MOVE_STEP;
                        }
                        if (upper_x - last_upper_x).abs() > CAMERA_MOVE_STEP {
                            upper_x = last_upper_x + (upper_x - last_upper_x) / (upper_x - last_upper_x).abs() * CAMERA_MOVE_STEP;
                        }
                        if (upper_y - last_upper_y).abs() > CAMERA_MOVE_STEP {
                            upper_y = last_upper_y + (upper_y - last_upper_y) / (upper_y - last_upper_y).abs() * CAMERA_MOVE_STEP;
                        }
                    }
                    last_lower_x = lower_x;
                    last_lower_y = lower_y;
                    last_upper_x = upper_x;
                    last_upper_y = upper_y;
    
                    let true_frame_height = upper_y - lower_y;
                    let true_frame_width = upper_x - lower_x;
    
                    for (origin, speed, mass) in frame_data {
                        let x = (origin.x - lower_x) / true_frame_width * FRAME_WIDTH as f64;
                        let y = (origin.y - lower_y) / true_frame_height * FRAME_WIDTH as f64;
    
                        let r = 0.4 + 0.6 * speed / max_vel;
                        let g = r;
                        let b = (r + g) / 2.0;
                        drawing.fill_color(Color::Rgba(r as f32, g as f32, b as f32, 1.0));

                        drawing.new_path();
                        drawing.circle(x as f32, y as f32, (mass.powf(1.0 / 3.0) * STAR_RADIUS_FACTOR) as f32);
                        drawing.fill();
                    }

                    let x = - lower_x / true_frame_width * FRAME_WIDTH as f64;
                    let y = - lower_y / true_frame_height * FRAME_WIDTH as f64;

                    drawing.fill_color(Color::Rgba(CENTER_CROSS_BRIGHTNESS, 0.0, 0.0, 1.0));
                    drawing.new_path();
                    drawing.rect(x as f32 - CENTER_RECT_HALF_WIDTH, y as f32 - CENTER_RECT_HALF_HEIGHT, x as f32 + CENTER_RECT_HALF_WIDTH, y as f32 + CENTER_RECT_HALF_HEIGHT);
                    drawing.fill();

                    drawing.new_path();
                    drawing.rect(x as f32 - CENTER_RECT_HALF_HEIGHT, y as f32 - CENTER_RECT_HALF_WIDTH, x as f32 + CENTER_RECT_HALF_HEIGHT, y as f32 + CENTER_RECT_HALF_WIDTH);
                    drawing.fill();

                    ui.draw(|gc| gc.extend(drawing.clone()));
                    let frame = block_on(render_canvas_offscreen(&mut canvas, FRAME_WIDTH, FRAME_WIDTH, 1.0, stream::iter(drawing)));
                    send_to_ffmpeg!(frame);
    
                    if i == 0 {
                        continue;
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
                }

                if let Err(e) = ffmpeg_stdin.flush() {
                    eprintln!("{e}");
                    std::io::copy(&mut ffmpeg_stderr, &mut stderr().lock()).expect("stderr copy to work");
                    exit(1);
                }

                drop(ffmpeg_stdin);
                let output = ffmpeg.wait_with_output().expect("FFMPEG process status failed to collect");
        
                if let Err(e) = output.status.exit_ok() {
                    println!("ffmpeg status: {e}");
                    stderr().write_all(&output.stderr).unwrap();
                    exit(1);
                }

                println!("Video done in {}s", start.elapsed().as_secs_f32());
                exit(0);
            },
        );
    });

    Ok(())
}
