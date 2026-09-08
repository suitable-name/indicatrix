//! Camera ray-generation GPU self-test: per-function ULP budget.
//!
//! Dispatches `shaders/camera_ray.wgsl` over a dense, independently-generated grid of
//! `(yaw, pitch, distance, fov_deg, screen_x, screen_y, width, height, jitter_x,
//! jitter_y)` cases -- including adversarial ones -- and compares every output ray's
//! `origin`/`dir` against `optics::raytracer::Camera::new` +
//! `Camera::generate_ray`, called directly (never re-derived) on the CPU.
//!
//! This is a fresh translation of `Camera::new`/`Camera::generate_ray` as they exist in
//! `raytracer.rs` today -- see that module's own doc comment on why any port is a fresh
//! translation, never a repair of the quarantined old shader.

use crate::{
    optics::raytracer::Camera,
    renderer::{
        buffers::GpuRay,
        gpu::{
            compute,
            ulp::{ulp_distance, within_tolerance},
        },
    },
};

const SHADER_SRC: &str = include_str!("../shaders/camera_ray.wgsl");

/// Must match `shaders/camera_ray.wgsl`'s `CameraRayCase` field-for-field.
#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct CameraRayCase {
    pub yaw: f32,
    pub pitch: f32,
    pub distance: f32,
    pub fov_deg: f32,
    pub screen_x: f32,
    pub screen_y: f32,
    pub width: f32,
    pub height: f32,
    pub jitter_x: f32,
    pub jitter_y: f32,
    _pad0: f32,
    _pad1: f32,
}

const _: () = assert!(size_of::<CameraRayCase>() == 48);

/// The `(yaw, pitch, distance, fov_deg)` a [`CameraRayCase`] feeds to `Camera::new`.
/// Grouped into its own small struct purely so [`CameraRayCase::new`] takes 2
/// arguments instead of 10.
#[derive(Clone, Copy)]
struct CameraPose {
    yaw: f32,
    pitch: f32,
    distance: f32,
    fov_deg: f32,
}

/// The `(screen_x, screen_y, width, height, jitter_x, jitter_y)` a [`CameraRayCase`]
/// feeds to `Camera::generate_ray`. See [`CameraPose`]'s doc comment for why this is
/// split out.
#[derive(Clone, Copy)]
struct ScreenSample {
    screen_x: f32,
    screen_y: f32,
    width: f32,
    height: f32,
    jitter_x: f32,
    jitter_y: f32,
}

impl CameraRayCase {
    #[must_use]
    const fn new(pose: CameraPose, screen: ScreenSample) -> Self {
        Self {
            yaw: pose.yaw,
            pitch: pose.pitch,
            distance: pose.distance,
            fov_deg: pose.fov_deg,
            screen_x: screen.screen_x,
            screen_y: screen.screen_y,
            width: screen.width,
            height: screen.height,
            jitter_x: screen.jitter_x,
            jitter_y: screen.jitter_y,
            _pad0: 0.0,
            _pad1: 0.0,
        }
    }

    /// The exact CPU reference ray for this case -- calls `Camera::new` +
    /// `Camera::generate_ray` directly, never a reimplementation.
    fn cpu_ray(self) -> crate::optics::raytracer::Ray {
        let camera = Camera::new(self.yaw, self.pitch, self.distance, self.fov_deg);
        camera.generate_ray(
            self.screen_x,
            self.screen_y,
            self.width,
            self.height,
            self.jitter_x,
            self.jitter_y,
        )
    }
}

/// Builds the dense + adversarial case grid.
///
/// Dense: a `9 (yaw) x 9 (pitch) x 3 (distance) x 4 (fov)` pose grid (972 poses), each
/// paired with 5 screen positions (four corners + center of a 64x64 frame) x 3 jitter
/// combinations (`{-0.499, 0.0, 0.499}` on both axes) = 15 ray cases per pose, for
/// 14,580 dense cases.
///
/// Adversarial (appended, not sampled from the grid):
/// - `pitch` within `1e-4` of `Camera::new`'s `world_up` branch boundary (`cos_p.abs() <
///   1e-4`) on both sides, at `yaw = 0` and `yaw = PI` -- the branch itself.
/// - `jitter_x`/`jitter_y` at the exact extremes the RNG-derived jitter can produce:
///   `-0.5` (`(0 / 10000.0) - 0.5`) and `0.4999` (`(9999 / 10000.0) - 0.5`).
/// - Screen coordinates exactly at the frame edges (`screen_x/y == 0` and `== width -
///   1`) with zero jitter, and exactly at the center pixel with jitter pushing the
///   sample past the pixel's nominal boundary.
/// - A very narrow (`fov_deg = 1.0`) and very wide (`fov_deg = 170.0`) field of view.
#[must_use]
pub fn build_cases() -> Vec<CameraRayCase> {
    let mut cases = dense_cases();
    cases.extend(adversarial_cases());
    cases
}

/// The dense `9 x 9 x 3 x 4` pose grid, each paired with 5 screen positions x 3 jitter
/// combinations -- see [`build_cases`]'s doc comment. Split out purely to keep
/// [`build_cases`] itself short; see [`adversarial_cases`] for the other half.
fn dense_cases() -> Vec<CameraRayCase> {
    let mut cases = Vec::new();

    let yaws: Vec<f32> = (0..9)
        .map(|i| (i as f32).mul_add(2.0 * std::f32::consts::PI / 8.0, -std::f32::consts::PI))
        .collect();
    let pitches: Vec<f32> = (0..9)
        .map(|i| (i as f32).mul_add(3.10 / 8.0, -1.55))
        .collect();
    let distances = [1.0f32, 3.0, 8.0];
    let fovs = [15.0f32, 45.0, 90.0, 140.0];
    let (width, height) = (64.0f32, 64.0f32);
    let screen_positions = [
        (0.0f32, 0.0f32),
        (63.0, 0.0),
        (0.0, 63.0),
        (63.0, 63.0),
        (32.0, 32.0),
    ];
    let jitters = [(-0.499f32, -0.499f32), (0.0, 0.0), (0.499, 0.499)];

    for &yaw in &yaws {
        for &pitch in &pitches {
            for &distance in &distances {
                for &fov_deg in &fovs {
                    for &(sx, sy) in &screen_positions {
                        for &(jx, jy) in &jitters {
                            cases.push(CameraRayCase::new(
                                CameraPose {
                                    yaw,
                                    pitch,
                                    distance,
                                    fov_deg,
                                },
                                ScreenSample {
                                    screen_x: sx,
                                    screen_y: sy,
                                    width,
                                    height,
                                    jitter_x: jx,
                                    jitter_y: jy,
                                },
                            ));
                        }
                    }
                }
            }
        }
    }

    cases
}

/// The three adversarial case groups appended to the dense grid -- see
/// [`build_cases`]'s doc comment for what each targets. Split out purely to keep
/// [`build_cases`] itself short; see [`dense_cases`] for the other half.
fn adversarial_cases() -> Vec<CameraRayCase> {
    let mut cases = Vec::new();

    // Adversarial: dead-on the world_up branch boundary.
    for &sign in &[1.0f32, -1.0] {
        for &yaw in &[0.0f32, std::f32::consts::PI] {
            for &eps in &[1e-5f32, -1e-5, 9e-5, -9e-5] {
                let pitch = f32::mul_add(sign, std::f32::consts::FRAC_PI_2, eps);
                cases.push(CameraRayCase::new(
                    CameraPose {
                        yaw,
                        pitch,
                        distance: 4.0,
                        fov_deg: 60.0,
                    },
                    ScreenSample {
                        screen_x: 32.0,
                        screen_y: 32.0,
                        width: 64.0,
                        height: 64.0,
                        jitter_x: 0.0,
                        jitter_y: 0.0,
                    },
                ));
            }
        }
    }

    // Adversarial: exact RNG-derived jitter extremes, at frame edges.
    for &(sx, sy) in &[(0.0f32, 0.0f32), (63.0, 63.0), (32.0, 32.0)] {
        for &(jx, jy) in &[(-0.5f32, -0.5f32), (0.4999, 0.4999), (-0.5, 0.4999)] {
            cases.push(CameraRayCase::new(
                CameraPose {
                    yaw: 0.6,
                    pitch: 0.4,
                    distance: 5.0,
                    fov_deg: 50.0,
                },
                ScreenSample {
                    screen_x: sx,
                    screen_y: sy,
                    width: 64.0,
                    height: 64.0,
                    jitter_x: jx,
                    jitter_y: jy,
                },
            ));
        }
    }

    // Adversarial: extreme fields of view.
    for &fov_deg in &[1.0f32, 170.0] {
        cases.push(CameraRayCase::new(
            CameraPose {
                yaw: 0.3,
                pitch: 0.2,
                distance: 5.0,
                fov_deg,
            },
            ScreenSample {
                screen_x: 32.0,
                screen_y: 32.0,
                width: 64.0,
                height: 64.0,
                jitter_x: 0.0,
                jitter_y: 0.0,
            },
        ));
    }

    cases
}

/// One case's worst-component ULP disagreement, kept only when it's the running argmax.
#[derive(Debug, Clone, Copy)]
pub struct CameraRayUlpArgmax {
    pub case_index: usize,
    pub case: CameraRayCase,
    pub component: &'static str,
    pub cpu: f32,
    pub gpu: f32,
    pub ulp: u32,
}

#[derive(Debug, Clone, Copy)]
pub struct CameraRayCheckResult {
    pub total_cases: usize,
    pub budget: u32,
    pub abs_floor: f32,
    /// Max ULP among GENUINE disagreements (excluding absolute-floor-exempted ones --
    /// see [`CAMERA_RAY_ABS_FLOOR`]'s doc comment).
    pub max_ulp: u32,
    /// Max ULP across EVERY comparison, exempted or not -- purely informational.
    pub max_raw_ulp: u32,
    pub argmax: Option<CameraRayUlpArgmax>,
    pub over_budget_count: usize,
    pub exempted_count: usize,
}

impl CameraRayCheckResult {
    #[must_use]
    pub const fn passed(&self) -> bool {
        self.over_budget_count == 0
    }
}

/// ULP budget for every component of the ported `Camera::new`/`Camera::generate_ray`
/// pipeline.
///
/// Covers both `origin` (a single `f32 * f32` multiply chain with no trig re-entry) and
/// `dir`, which chains `cos`/`sin`/`tan`/`normalize`/`cross` through several
/// intermediate roundings.
///
/// # Where this number comes from
///
/// Measured on this workspace's dev hardware (AMD Radeon 680M-class RDNA2 iGPU, Vulkan
/// backend) over the case grid in [`build_cases`] (14,600+ cases, 6 components each):
/// see the harness output for the actual measured max. `rng_check::FLOAT_ULP_BUDGET`'s
/// doc comment established the calibration this budget reuses: driver float noise
/// (differing `fma`/transcendental lowering between the CPU's libm and the GPU shader
/// compiler) measures at 1-2 ULP per operation; a chain of several such operations
/// (`cos`, `sin`, `tan`, two `normalize`s, two `cross`es) can accumulate a handful more
/// without indicating a porting bug. 64 ULP is generous enough to absorb that
/// accumulation across the whole `Camera::new` -> `generate_ray` chain on a different
/// GPU/driver, while staying many orders of magnitude below the injected-fault
/// magnitude a real algebra error produces (calibration: 8,552,444 ULP for a
/// deliberately wrong formula) -- see this crate's negative-control run for an instance
/// of that same magnitude gap.
pub const CAMERA_RAY_ULP_BUDGET: u32 = 64;

/// Absolute-difference floor exempting comparisons near a trig zero-crossing from
/// [`CAMERA_RAY_ULP_BUDGET`] entirely.
///
/// See
/// [`crate::renderer::gpu::ulp::within_tolerance`]'s doc comment for the general
/// rationale). Measured concretely on this workspace's dev hardware: `yaw = PI` (an
/// adversarial case in [`build_cases`]) makes `origin.x = distance * cos(pitch) *
/// sin(yaw)`, and `sin(PI_as_f32)` is only ~8.7e-8 away from its mathematically exact
/// (but unrepresentable) zero to begin with -- the CPU and GPU trig implementations
/// round that already-tiny value to *opposite sides* of exactly `0.0`, which an
/// ULP-distance-through-zero metric reports as billions of ULP even though the absolute
/// disagreement is a few millionths. `1e-4` stays far below any position/direction
/// component's meaningful scale in this crate's scenes (camera distances of 1-8 units,
/// unit-length directions) while comfortably covering that measured noise floor.
pub const CAMERA_RAY_ABS_FLOOR: f32 = 1e-4;

/// Runs the camera ray-generation self-test against a live GPU.
///
/// # Panics
///
/// Panics on `wgpu` API misuse (see [`crate::renderer::gpu::layout_check::run`]'s doc
/// comment for the same rationale).
#[must_use]
pub fn run(ctx: &crate::renderer::gpu::GpuContext) -> CameraRayCheckResult {
    let cases = build_cases();
    let total = cases.len();

    let cases_buf = compute::upload(
        &ctx.device,
        "camera_ray cases",
        &cases,
        wgpu::BufferUsages::STORAGE,
    );
    let out_buf = compute::zeroed_buffer::<GpuRay>(
        &ctx.device,
        "camera_ray output",
        total,
        wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
    );

    let pipeline = compute::create_compute_pipeline(&ctx.device, "camera_ray", SHADER_SRC, "main");
    let bind_group = compute::bind_buffers(
        &ctx.device,
        "camera_ray bind group",
        &pipeline,
        &[(0, &cases_buf), (1, &out_buf)],
    );

    let workgroups = (total as u32).div_ceil(64);
    compute::dispatch_and_wait(
        &ctx.device,
        &ctx.queue,
        &pipeline,
        &bind_group,
        (workgroups, 1, 1),
    );

    let gpu_rays: Vec<GpuRay> = compute::readback(&ctx.device, &ctx.queue, &out_buf, total);

    let mut max_ulp = 0u32;
    let mut max_raw_ulp = 0u32;
    let mut argmax = None;
    let mut over_budget_count = 0usize;
    let mut exempted_count = 0usize;

    for (idx, (&case, gpu_ray)) in cases.iter().zip(gpu_rays.iter()).enumerate() {
        let cpu_ray = case.cpu_ray();
        let components: [(&'static str, f32, f32); 6] = [
            ("origin.x", cpu_ray.origin.x, gpu_ray.origin[0]),
            ("origin.y", cpu_ray.origin.y, gpu_ray.origin[1]),
            ("origin.z", cpu_ray.origin.z, gpu_ray.origin[2]),
            ("dir.x", cpu_ray.dir.x, gpu_ray.dir[0]),
            ("dir.y", cpu_ray.dir.y, gpu_ray.dir[1]),
            ("dir.z", cpu_ray.dir.z, gpu_ray.dir[2]),
        ];
        for (component, cpu, gpu) in components {
            let ulp = ulp_distance(cpu, gpu);
            if ulp > max_raw_ulp {
                max_raw_ulp = ulp;
            }
            if within_tolerance(cpu, gpu, CAMERA_RAY_ULP_BUDGET, CAMERA_RAY_ABS_FLOOR) {
                if ulp > CAMERA_RAY_ULP_BUDGET {
                    exempted_count += 1;
                }
            } else {
                over_budget_count += 1;
                if ulp > max_ulp {
                    max_ulp = ulp;
                    argmax = Some(CameraRayUlpArgmax {
                        case_index: idx,
                        case,
                        component,
                        cpu,
                        gpu,
                        ulp,
                    });
                }
            }
        }
    }

    CameraRayCheckResult {
        total_cases: total,
        budget: CAMERA_RAY_ULP_BUDGET,
        abs_floor: CAMERA_RAY_ABS_FLOOR,
        max_raw_ulp,
        exempted_count,
        max_ulp,
        argmax,
        over_budget_count,
    }
}
