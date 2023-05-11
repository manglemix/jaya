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
// use image::{codecs::gif::GifEncoder, Delay, Frame, ImageBuffer, Rgba};
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

const FPS: usize = 120;
const DELTA: f64 = 1.0 / FPS as f64;

const FRAME_WIDTH: usize = 600;
const SPEED_BOUNDS: f64 = 0.25;

const MIN_MASS: f64 = 100.0;
const MAX_MASS: f64 = 10000.0;

const MIN_DISTANCE: f64 = 5.0;

const MIN_AGE: f64 = 0.75;
const MAX_AGE: f64 = 100.0;

const FRAME_COUNT: usize = (30.0 * FPS as f32) as usize;
const PARTICLE_COUNT: usize = 1500;

const FRAME_QUANTILE: usize = 5;

const SQUARE_PIXELS_PER_PARTICLE: usize = 600;
const SPAWN_RADIUS_SQUARED: f64 = (SQUARE_PIXELS_PER_PARTICLE * PARTICLE_COUNT) as f64 / PI;

const CENTER_RECT_HALF_WIDTH: f32 = 1.0;
const CENTER_RECT_HALF_HEIGHT: f32 = 4.0;
const CENTER_CROSS_BRIGHTNESS: f32 = 0.3;
const STAR_RADIUS_FACTOR: f64 = 0.1;

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
    const FORCE_FACTOR: f64 = 3.0;
    const AGE_FACTOR: f64 = 2.0;
    if p1.age > DELTA {
        p1.queue_mut(|p1| p1.age -= DELTA);
        return;
    }

    if fastrand::bool() {
        let extra_vel = Rotation2::new(PI / 2.0) * p1.velocity;
        p1.queue_mut(move |p1| {
            p1.age = MIN_AGE + fastrand::f64() * (MAX_AGE - MIN_AGE);
            p1.velocity += extra_vel;
        });
        universe.add_entity((ParticleComponent {
            origin: p1.origin,
            velocity: p1.velocity - extra_vel,
            mass: p1.mass,
            age: MIN_AGE + fastrand::f64() * (MAX_AGE - MIN_AGE)
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
            p2.age -= age_factor;
        });
    };
    universe.queue_remove_entity(p1.get_id());
    explode.run_once(universe);
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

    let (frame_sender, frame_receiver) = sync_channel::<Vec<ParticleComponent>>(FRAME_COUNT);

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
            || {
                let start = Instant::now();
        
                for _i in 0..FRAME_COUNT {
                    join(
                        || {
                            (attraction, ageing).run_once(&particles);
                        },
                        || {
                            let frame_data: Vec<_> = particles
                                .iter_components_collect(|_, x: &ParticleComponent| *x);
                            frame_sender.send(frame_data).unwrap();
                        }
                    );
                    
                    particles.process_queues();
                }
    
                drop(frame_sender);
                println!("Simulation done in {}s", start.elapsed().as_secs_f32());
            },
            || {
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

                frame_receiver.into_iter().enumerate().for_each(|(i, frame_data)| {
                    let mut drawing = vec![];
                    drawing.clear_canvas(Color::Rgba(0.0, 0.0, 0.0, 1.0));
                    drawing.canvas_height(FRAME_WIDTH as f32);
                    drawing.center_region(0.0, 0.0, FRAME_WIDTH as f32, FRAME_WIDTH as f32);

                    if frame_data.is_empty() {
                        ui.draw(|gc| gc.extend(drawing.clone()));
                        let frame = block_on(render_canvas_offscreen(&mut canvas, FRAME_WIDTH, FRAME_WIDTH, 1.0, stream::iter(drawing)));
                        send_to_ffmpeg!(frame);
                        return;
                    }

                    let max_vel = frame_data.iter().map(|p| FloatOrd(p.velocity.magnitude())).max().unwrap().0;
                    let mut origins: Vec<_> = frame_data.iter().map(|p| p.origin).collect();
                    let center = origins.iter().sum::<Vector2<f64>>().scale(1.0 / frame_data.len() as f64);
    
                    origins.par_sort_unstable_by_key(|origin| FloatOrd(origin.x - center.x));
                    let lower_x = origins.get(origins.len() / FRAME_QUANTILE).unwrap().x;
                    let upper_x = origins.get(origins.len() * (FRAME_QUANTILE - 1) / FRAME_QUANTILE).unwrap().x;
    
                    origins.par_sort_unstable_by_key(|origin| FloatOrd(origin.y - center.y));
                    let lower_y = origins.get(origins.len() / FRAME_QUANTILE).unwrap().y;
                    let upper_y = origins.get(origins.len() * (FRAME_QUANTILE - 1) / FRAME_QUANTILE).unwrap().y;
    
                    let true_frame_height = upper_y - lower_y;
                    let true_frame_width = upper_x - lower_x;
    
                    for p in frame_data {
                        let x = (p.origin.x - lower_x) / true_frame_width * FRAME_WIDTH as f64;
                        let y = (p.origin.y - lower_y) / true_frame_height * FRAME_WIDTH as f64;
    
                        if x >= 0.0 && y >= 0.0 {
                            let r = 0.4 + 0.6 * p.velocity.magnitude() / max_vel;
                            let g = r;
                            let b = (r + g) / 2.0;
                            drawing.fill_color(Color::Rgba(r as f32, g as f32, b as f32, 1.0));

                            drawing.new_path();
                            drawing.circle(x as f32, y as f32, (p.mass.powf(1.0 / 3.0) * STAR_RADIUS_FACTOR) as f32);
                            drawing.fill();
                        }
                    }

                    let x = (-center.x - lower_x) / true_frame_width * FRAME_WIDTH as f64;
                    let y = (-center.y - lower_y) / true_frame_height * FRAME_WIDTH as f64;

                    drawing.fill_color(Color::Rgba(CENTER_CROSS_BRIGHTNESS, 0.0, 0.0, 1.0));
                    drawing.new_path();
                    drawing.rect(x as f32 - CENTER_RECT_HALF_WIDTH, y as f32 - CENTER_RECT_HALF_HEIGHT, x as f32 + CENTER_RECT_HALF_WIDTH, y as f32 + CENTER_RECT_HALF_HEIGHT);
                    drawing.fill();

                    drawing.new_path();
                    drawing.rect(x as f32 - CENTER_RECT_HALF_HEIGHT, y as f32 - CENTER_RECT_HALF_WIDTH, x as f32 + CENTER_RECT_HALF_HEIGHT, y as f32 + CENTER_RECT_HALF_WIDTH);
                    drawing.fill();

                    ui.draw(|gc| gc.extend(drawing.clone()));
                    let frame = block_on(render_canvas_offscreen(&mut canvas, 600, 600, 1.0, stream::iter(drawing)));
                    send_to_ffmpeg!(frame);
    
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
    // let output = Command::new("ffmpeg")
    //     .args("-i simulation.gif -movflags faststart -pix_fmt yuv420p -vf scale=800:800:flags=neighbor,setpts=PTS/12,fps=120 -b:v 8M simulation.mp4 -y".split_ascii_whitespace())
    //     .output()
    //     .unwrap();

    Ok(())
}
