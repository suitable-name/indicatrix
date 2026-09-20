//! A PNG gallery of the lighting presets, for judging the look by eye rather than by
//! assertion: the standard round brilliant in Diamond, Rutile and Sapphire, face-up
//! and tilted, under the studio ring lights (the old look, as a reference) and the
//! three lit models, plus the light tent's free parameters (exposure, key-light
//! elevation). Every image has `GemRay`'s grey backdrop card behind the stone.
//!
//! The gallery renders on the GPU through the production frame renderer, at full
//! size and a high sample count, so it is ignored by default and takes hours:
//!
//! ```text
//! cargo test -p indicatrix --release --features gpu --test lighting_gallery -- --ignored --nocapture
//! ```
//!
//! It is resumable: every finished image is written at once, an image already on disk
//! is kept rather than re-rendered (delete it, or set `INDICATRIX_GALLERY_FORCE=1`, to
//! redo it), and a lost GPU device -- the renderer's 4 s watchdog trips when a shared
//! integrated GPU stalls, not only on a real driver crash -- re-acquires the GPU and
//! starts that one image over instead of failing the run.
//!
//! `INDICATRIX_GALLERY_SIZE` (pixels, square) and `INDICATRIX_GALLERY_SPP` (samples per
//! pixel) override the defaults. Images land in `target/tmp/lighting_gallery/`
//! (`CARGO_TARGET_TMPDIR`); the test prints the directory and the seconds each image
//! took, and `index.txt` there lists every image with its settings and what to look
//! for. The small non-ignored test at the bottom keeps the shot description, tone map
//! and PNG encoder exercised on every ordinary test run, on the CPU, at postage-stamp
//! size.

use glam::Vec3;
use indicatrix::{
    geometry::cuts::StandardGemCuts,
    optics::{
        materials::GemMaterial,
        raytracer::{
            BACKDROP_GREY, Camera, EnvironmentSource, LightingPreset, build_plane_soa, hash_u32,
            trace_spectral_ray_with_finish_soa, xyz_to_srgb_gamma,
        },
    },
};
use std::f32::consts::FRAC_PI_2;

const MAX_BOUNCES: u32 = 12;
/// The editor's own field of view and default camera distance, so the gallery matches
/// what the Live Render tab shows.
const FOV_DEG: f32 = 42.0;
const CAMERA_DISTANCE: f32 = 2.4;
/// The editor's default key-light pose (azimuth / elevation in degrees).
const DEFAULT_LIGHT_YAW_DEG: f32 = 48.0;
const DEFAULT_LIGHT_PITCH_DEG: f32 = 54.0;

#[derive(Clone, Copy)]
struct View {
    name: &'static str,
    yaw: f32,
    pitch: f32,
}

/// Straight down onto the table -- the batch previews' own top pose.
const TOP: View = View {
    name: "top",
    yaw: 0.0,
    pitch: FRAC_PI_2,
};

#[derive(Clone, Copy)]
struct Shot {
    material: &'static str,
    view: View,
    preset: LightingPreset,
    exposure: f32,
    light_yaw_deg: f32,
    light_pitch_deg: f32,
}

impl Shot {
    const fn standard(material: &'static str, view: View, preset: LightingPreset) -> Self {
        Self {
            material,
            view,
            preset,
            exposure: 1.0,
            light_yaw_deg: DEFAULT_LIGHT_YAW_DEG,
            light_pitch_deg: DEFAULT_LIGHT_PITCH_DEG,
        }
    }

    fn file_name(&self) -> String {
        format!(
            "{}_{}_{}_exp{:.1}_light{:.0}-{:.0}.png",
            slug(self.material),
            self.view.name,
            slug(self.preset.label()),
            self.exposure,
            self.light_yaw_deg,
            self.light_pitch_deg
        )
    }

    fn material(&self) -> GemMaterial {
        GemMaterial::by_name(self.material)
            .unwrap_or_else(|| panic!("{} is a built-in material", self.material))
    }

    fn camera(&self) -> Camera {
        Camera::new(self.view.yaw, self.view.pitch, CAMERA_DISTANCE, FOV_DEG)
    }

    /// The scene's environment, with `GemRay`'s grey card behind the stone so the
    /// gallery compares like for like with `GemRay`'s own pictures.
    const fn environment(&self) -> EnvironmentSource<'static> {
        self.preset
            .studio(
                self.exposure,
                self.light_yaw_deg.to_radians(),
                self.light_pitch_deg.to_radians(),
            )
            .with_backdrop(BACKDROP_GREY)
    }
}

/// Lower-case, non-alphanumerics collapsed to single dashes: a label as a file name.
fn slug(label: &str) -> String {
    let mut out = String::new();
    for c in label.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    out.trim_matches('-').to_string()
}

/// Averages accumulated XYZ sums into tightly packed 8-bit sRGB.
fn encode_rgb(accum: &[Vec3], samples_per_pixel: u32) -> Vec<u8> {
    let inv = 1.0 / samples_per_pixel as f32;
    accum
        .iter()
        .flat_map(|xyz| {
            let rgba = xyz_to_srgb_gamma(*xyz * inv);
            [rgba[0], rgba[1], rgba[2]]
        })
        .collect()
}

/// Renders `shot` on the CPU, rows split across every available core -- the smoke
/// test's path.
fn render_cpu(shot: &Shot, size: u32, samples_per_pixel: u32) -> Vec<u8> {
    let planes = StandardGemCuts::standard_round_brilliant();
    let plane_soa = build_plane_soa(&planes);
    let material = shot.material();
    let camera = shot.camera();
    let environment = shot.environment();
    let shot_salt = shot
        .file_name()
        .bytes()
        .fold(0x9E37_79B9u32, |h, b| hash_u32(h ^ u32::from(b)));
    let threads = std::thread::available_parallelism().map_or(4, usize::from);
    let dim = size as f32;

    let mut accum = vec![Vec3::ZERO; (size * size) as usize];
    std::thread::scope(|scope| {
        // Spawn every worker before joining any, so the rows really render in parallel.
        let mut handles = Vec::with_capacity(threads);
        for t in 0..threads {
            let (planes, plane_soa, material, camera) = (&planes, &plane_soa, &material, &camera);
            handles.push(scope.spawn(move || {
                let mut thread_rows = Vec::new();
                for y in (t as u32..size).step_by(threads) {
                    let mut row = Vec::with_capacity(size as usize);
                    for x in 0..size {
                        let pixel = y * size + x;
                        let mut xyz = Vec3::ZERO;
                        for s in 0..samples_per_pixel {
                            let seed =
                                hash_u32(shot_salt ^ hash_u32(pixel ^ hash_u32(s ^ 0x51ED_270B)));
                            let jitter_x = hash_u32(seed ^ 0xA511_E9B3) as f32 / 4_294_967_295.0;
                            let jitter_y = hash_u32(seed ^ 0x63D8_3595) as f32 / 4_294_967_295.0;
                            let hero_rand = hash_u32(seed) as f32 / 4_294_967_295.0;
                            let ray = camera
                                .generate_ray(x as f32, y as f32, dim, dim, jitter_x, jitter_y);
                            xyz += trace_spectral_ray_with_finish_soa(
                                ray,
                                planes,
                                plane_soa,
                                &[],
                                material,
                                MAX_BOUNCES,
                                environment,
                                seed,
                                hero_rand,
                                None,
                            );
                        }
                        row.push(xyz);
                    }
                    thread_rows.push((y, row));
                }
                thread_rows
            }));
        }
        for handle in handles {
            for (y, row) in handle.join().expect("render thread panicked") {
                let start = (y * size) as usize;
                accum[start..start + size as usize].copy_from_slice(&row);
            }
        }
    });
    encode_rgb(&accum, samples_per_pixel)
}

/// Encodes tightly packed 8-bit RGB as a PNG: uncompressed (stored) deflate blocks, no
/// filtering, so the test needs no image dependency.
fn png_bytes(width: u32, height: u32, rgb: &[u8]) -> Vec<u8> {
    assert_eq!(rgb.len(), (width * height * 3) as usize);
    let mut raw = Vec::with_capacity(rgb.len() + height as usize);
    for row in rgb.chunks_exact(width as usize * 3) {
        raw.push(0); // filter type: none
        raw.extend_from_slice(row);
    }

    let mut zlib = vec![0x78, 0x01];
    let block_count = raw.len().div_ceil(65_535).max(1);
    for (i, block) in raw.chunks(65_535).enumerate() {
        zlib.push(u8::from(i + 1 == block_count)); // BFINAL, BTYPE = stored
        let len = block.len() as u16;
        zlib.extend_from_slice(&len.to_le_bytes());
        zlib.extend_from_slice(&(!len).to_le_bytes());
        zlib.extend_from_slice(block);
    }
    zlib.extend_from_slice(&adler32(&raw).to_be_bytes());

    let mut ihdr = Vec::with_capacity(13);
    ihdr.extend_from_slice(&width.to_be_bytes());
    ihdr.extend_from_slice(&height.to_be_bytes());
    ihdr.extend_from_slice(&[8, 2, 0, 0, 0]); // 8-bit RGB, deflate, no filter, no interlace

    let mut png = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    push_chunk(&mut png, *b"IHDR", &ihdr);
    push_chunk(&mut png, *b"IDAT", &zlib);
    push_chunk(&mut png, *b"IEND", &[]);
    png
}

fn push_chunk(png: &mut Vec<u8>, kind: [u8; 4], data: &[u8]) {
    png.extend_from_slice(&(data.len() as u32).to_be_bytes());
    let start = png.len();
    png.extend_from_slice(&kind);
    png.extend_from_slice(data);
    let crc = crc32(&png[start..]);
    png.extend_from_slice(&crc.to_be_bytes());
}

fn crc32(bytes: &[u8]) -> u32 {
    let mut c = 0xFFFF_FFFFu32;
    for &b in bytes {
        c ^= u32::from(b);
        for _ in 0..8 {
            c = if c & 1 == 1 {
                0xEDB8_8320 ^ (c >> 1)
            } else {
                c >> 1
            };
        }
    }
    !c
}

fn adler32(bytes: &[u8]) -> u32 {
    let (mut a, mut b) = (1u32, 0u32);
    for &byte in bytes {
        a = (a + u32::from(byte)) % 65_521;
        b = (b + a) % 65_521;
    }
    (b << 16) | a
}

/// The GPU gallery itself: the shot list, the production frame renderer, the index.
#[cfg(feature = "gpu")]
mod gallery {
    use super::{MAX_BOUNCES, Shot, TOP, Vec3, View, encode_rgb, png_bytes};
    use indicatrix::{
        geometry::cuts::StandardGemCuts,
        optics::raytracer::LightingPreset,
        renderer::gpu::{GpuFrameError, GpuFrameRenderer, GpuFrameScene},
    };
    use std::{
        fs,
        path::{Path, PathBuf},
        time::{Duration, Instant},
    };

    /// Image size in pixels (square) unless `INDICATRIX_GALLERY_SIZE` says otherwise.
    const GALLERY_SIZE: u32 = 1024;
    /// Samples per pixel unless `INDICATRIX_GALLERY_SPP` says otherwise -- enough that
    /// the sun and spark flashes converge instead of reading as noise.
    const GALLERY_SAMPLES_PER_PIXEL: u32 = 4096;
    /// Samples per `GpuFrameRenderer::accumulate` call; the renderer chunks pixels
    /// within a call by its byte budget, so this only bounds how often progress can be
    /// reported.
    const GPU_BATCH_SPP: u32 = 128;
    /// How often one image is started over after a lost device before the run gives
    /// up (a rerun resumes from the images already on disk), and the pause before each
    /// re-acquisition so a resetting driver has time to come back.
    const RECOVERY_ATTEMPTS: u32 = 6;
    const RECOVERY_PAUSE: Duration = Duration::from_secs(10);

    /// The editor's default camera pose.
    const TILTED: View = View {
        name: "tilted",
        yaw: 0.60,
        pitch: 0.45,
    };

    const INDEX_HEADER: &str = "\
Lighting gallery -- the standard round brilliant on GemRay's grey backdrop card,
rendered on the GPU by crates/indicatrix/tests/lighting_gallery.rs. Look for:
  * Gem Studio Ring Lights (reference, the old look): black stone with hard white
    flashes and rainbow speckle, nothing in between.
  * ISO hemisphere: evenly white facets, black leakage, and the dark
    head-shadow pattern in the table -- the GemRay ISO picture.
  * Light tent + black cards: grey-to-white gradation across the facets, black card
    reflections for contrast, one hard spark; Rutile keeps a saturated yellow body
    colour with dark facets instead of a pale wash; Sapphire stays deep blue.
  * Daylight sky + sun: grey sky facets, white sun flashes with rainbow fringes,
    dark head shadow, dark ground.
The exposure and key-light elevation variants show how far the light tent moves with
the two controls the editor exposes.

";

    fn describe(shot: &Shot) -> String {
        format!(
            "{}  ->  {} on the standard round brilliant, {} view (camera yaw {:.2} rad, pitch {:.2} rad), \
             lighting \"{}\", exposure {:.1}x, key light azimuth {:.0} deg / elevation {:.0} deg",
            shot.file_name(),
            shot.material,
            shot.view.name,
            shot.view.yaw,
            shot.view.pitch,
            shot.preset.label(),
            shot.exposure,
            shot.light_yaw_deg,
            shot.light_pitch_deg
        )
    }

    fn gallery_shots() -> Vec<Shot> {
        let presets = [
            LightingPreset::RingLights,
            LightingPreset::IsoHemisphere,
            LightingPreset::LightTent,
            LightingPreset::DaylightDome,
        ];
        let mut shots = Vec::new();
        for material in ["Diamond", "Rutile"] {
            for view in [TOP, TILTED] {
                for preset in presets {
                    shots.push(Shot::standard(material, view, preset));
                }
            }
        }
        // A saturated coloured stone, tilted, under the two presentation models.
        for preset in [LightingPreset::LightTent, LightingPreset::DaylightDome] {
            shots.push(Shot::standard("Sapphire", TILTED, preset));
        }
        // The light tent's free parameters, on the diamond's tilted view.
        let tent = Shot::standard("Diamond", TILTED, LightingPreset::LightTent);
        for exposure in [0.6, 1.6] {
            shots.push(Shot { exposure, ..tent });
        }
        for light_pitch_deg in [35.0, 75.0] {
            shots.push(Shot {
                light_pitch_deg,
                ..tent
            });
        }
        shots.push(Shot {
            exposure: 0.6,
            ..Shot::standard("Rutile", TILTED, LightingPreset::LightTent)
        });
        shots
    }

    /// Renders `shot` through the production GPU frame renderer -- the same megakernel
    /// the editor's Live Render tab and the worker use.
    fn render_gpu(
        renderer: &mut GpuFrameRenderer,
        shot: &Shot,
        size: u32,
        samples_per_pixel: u32,
    ) -> Result<Vec<u8>, GpuFrameError> {
        let planes = StandardGemCuts::standard_round_brilliant();
        let material = shot.material();
        let camera = shot.camera();
        let scene = GpuFrameScene {
            camera: &camera,
            width: size,
            height: size,
            planes: &planes,
            facet_finishes: &[],
            material: &material,
            max_bounces: MAX_BOUNCES,
            environment: shot.environment(),
        };
        let mut accum = vec![Vec3::ZERO; (size * size) as usize];
        let mut done = 0u32;
        while done < samples_per_pixel {
            let batch = GPU_BATCH_SPP.min(samples_per_pixel - done);
            renderer.accumulate(&scene, done, batch, &mut accum)?;
            done += batch;
        }
        Ok(encode_rgb(&accum, samples_per_pixel))
    }

    /// Opens the GPU, retrying with pauses -- after a driver reset the adapter can be
    /// unavailable for a few seconds.
    fn acquire_gpu() -> GpuFrameRenderer {
        let mut last_error = None;
        for attempt in 1..=RECOVERY_ATTEMPTS {
            match GpuFrameRenderer::new() {
                Ok(renderer) => {
                    println!("GPU: {}", renderer.adapter_label());
                    return renderer;
                }
                Err(e) => {
                    eprintln!("no GPU yet (attempt {attempt}/{RECOVERY_ATTEMPTS}): {e}");
                    last_error = Some(e);
                    std::thread::sleep(RECOVERY_PAUSE);
                }
            }
        }
        panic!("the gallery renders on the GPU and none could be acquired: {last_error:?}")
    }

    /// [`render_gpu`], starting the image over on a fresh device after a lost one. The
    /// old renderer is dropped before the new one is opened so the lost device is
    /// released first; the partial accumulation is discarded, since nothing says which
    /// batch the loss interrupted.
    fn render_gpu_resilient(
        renderer: &mut Option<GpuFrameRenderer>,
        shot: &Shot,
        size: u32,
        samples_per_pixel: u32,
    ) -> Vec<u8> {
        let mut losses = 0u32;
        loop {
            let live = renderer.get_or_insert_with(acquire_gpu);
            match render_gpu(live, shot, size, samples_per_pixel) {
                Ok(rgb) => return rgb,
                Err(e) => {
                    losses += 1;
                    *renderer = None;
                    assert!(
                        losses < RECOVERY_ATTEMPTS,
                        "gave up on {} after {losses} lost devices ({e}); rerun the gallery to \
                         resume from the images already written",
                        shot.file_name()
                    );
                    eprintln!(
                        "{}: {e}\n  -> re-acquiring the GPU in {} s and starting the image over \
                         (loss {losses}/{RECOVERY_ATTEMPTS})",
                        shot.file_name(),
                        RECOVERY_PAUSE.as_secs()
                    );
                    std::thread::sleep(RECOVERY_PAUSE);
                }
            }
        }
    }

    fn gallery_dir() -> PathBuf {
        Path::new(env!("CARGO_TARGET_TMPDIR")).join("lighting_gallery")
    }

    fn env_or(name: &str, default: u32) -> u32 {
        std::env::var(name)
            .ok()
            .and_then(|value| value.parse().ok())
            .unwrap_or(default)
    }

    #[test]
    #[ignore = "PNG gallery for judging the lighting by eye, hours on the GPU and resumable; run with: cargo test -p indicatrix --release --features gpu --test lighting_gallery -- --ignored --nocapture"]
    fn render_lighting_gallery() {
        let size = env_or("INDICATRIX_GALLERY_SIZE", GALLERY_SIZE);
        let samples_per_pixel = env_or("INDICATRIX_GALLERY_SPP", GALLERY_SAMPLES_PER_PIXEL);
        let force = std::env::var_os("INDICATRIX_GALLERY_FORCE").is_some();
        println!("{size}x{size} px, {samples_per_pixel} spp, {MAX_BOUNCES} bounces");
        let mut renderer: Option<GpuFrameRenderer> = None;

        let dir = gallery_dir();
        fs::create_dir_all(&dir).expect("gallery directory must be creatable");
        let mut index = String::from(INDEX_HEADER);
        let shots = gallery_shots();
        let (mut rendered, mut kept) = (0usize, 0usize);
        for (n, shot) in shots.iter().enumerate() {
            let path = dir.join(shot.file_name());
            if path.exists() && !force {
                kept += 1;
                println!("[{}/{}] kept {}", n + 1, shots.len(), path.display());
            } else {
                let started = Instant::now();
                let rgb = render_gpu_resilient(&mut renderer, shot, size, samples_per_pixel);
                fs::write(&path, png_bytes(size, size, &rgb))
                    .expect("gallery image must be writable");
                rendered += 1;
                println!(
                    "[{}/{}] wrote {} ({:.0} s)",
                    n + 1,
                    shots.len(),
                    path.display(),
                    started.elapsed().as_secs_f32()
                );
            }
            index.push_str(&describe(shot));
            index.push('\n');
            // Rewritten after every image, so an interrupted run still leaves a complete
            // index for what is on disk.
            fs::write(dir.join("index.txt"), &index).expect("gallery index must be writable");
        }
        println!(
            "gallery: {} ({rendered} rendered, {kept} kept from an earlier run)",
            dir.display()
        );
    }
}

#[cfg(not(feature = "gpu"))]
#[test]
#[ignore = "PNG gallery for judging the lighting by eye; it renders on the GPU, so build with --features gpu"]
fn render_lighting_gallery() {
    panic!(
        "the lighting gallery renders on the GPU: cargo test -p indicatrix --release \
         --features gpu --test lighting_gallery -- --ignored --nocapture"
    );
}

/// The gallery pipeline end to end at postage-stamp size on the CPU: a valid PNG, and
/// a face-up diamond in the light tent shows both dark and bright pixels.
#[test]
fn light_tent_render_is_a_valid_png_with_contrast() {
    let shot = Shot::standard("Diamond", TOP, LightingPreset::LightTent);
    let rgb = render_cpu(&shot, 24, 2);
    let png = png_bytes(24, 24, &rgb);
    assert_eq!(&png[..8], &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]);
    assert!(png.ends_with(&[b'I', b'E', b'N', b'D', 0xAE, 0x42, 0x60, 0x82]));

    let (pixels, _) = rgb.as_chunks::<3>();
    let (min, max) = pixels
        .iter()
        .map(|p| p.iter().copied().max().unwrap_or(0))
        .fold((u8::MAX, u8::MIN), |(lo, hi), v| (lo.min(v), hi.max(v)));
    assert!(
        max - min > 60,
        "a face-up diamond in the light tent must show dark and bright facets (min {min}, max {max})"
    );
}
