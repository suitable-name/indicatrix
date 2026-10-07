//! The render job file: what a job freezes, and the rules a job must satisfy.
//!
//! A job is [`indicatrix_net::SceneState`] (the fully resolved scene, the same type a
//! remote worker receives) plus the export settings and the output location. The remote
//! worker address, its certificates and the local engine choice are machine settings,
//! not render content, so they are deliberately not part of a job.

use crate::codec::JobFileError;
use indicatrix_net::SceneState;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// The job file format this version writes and reads.
pub const JOB_FORMAT_VERSION: u32 = 1;
/// The suffix of a job file name.
pub const JOB_FILE_SUFFIX: &str = ".job.json";
/// The smallest picture side, in pixels.
pub const MIN_DIM: u32 = 16;
/// The largest picture side, in pixels.
pub const MAX_DIM: u32 = 8192;
/// The largest samples-per-pixel count.
pub const MAX_SPP: u32 = 32_768;
/// The largest bounce limit.
pub const MAX_BOUNCES: u32 = 128;
/// The largest number of frames in a tilt video.
pub const MAX_FRAMES: u32 = 18_001;
/// The largest video frame rate.
pub const MAX_FPS: u32 = 120;
/// The number of values in each metric curve (one per degree from -90 to 90).
pub const CURVE_POINTS: usize = 181;

/// A frozen render job: everything needed to render one picture or one video.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RenderJobFile {
    /// The job file format, [`JOB_FORMAT_VERSION`].
    pub format: u32,
    /// A random token of 32 lowercase hexadecimal characters, made when the job is added.
    pub token: String,
    /// The one-line name shown in the job list.
    pub label: String,
    /// When the job was added, in unix seconds. Informational.
    pub created_at: i64,
    /// Where the design came from. Informational.
    #[serde(default)]
    pub design: DesignInfo,
    /// The fully resolved scene: material, planes, lighting and camera.
    pub scene: SceneState,
    /// The HDR map file, exactly when the scene is lit by one.
    #[serde(default)]
    pub hdr: Option<HdrSource>,
    /// Which computers render, and how their results travel.
    pub compute: JobCompute,
    /// A still: the PNG path. A video: the frame folder. A relative path is taken
    /// against the folder of the job file.
    pub output: String,
    /// What the job renders.
    pub kind: JobKind,
}

/// Informational facts about the design a job was made from.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct DesignInfo {
    /// The design title.
    pub title: String,
    /// The designer.
    pub designer: String,
    /// The shape name.
    pub shape: String,
    /// The refractive index, as text.
    pub ri: String,
    /// The material name.
    pub material: String,
}

/// An HDR map file and the hash its bytes must have.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HdrSource {
    /// The file path. A relative path is taken against the folder of the job file.
    pub path: String,
    /// The SHA-256 of the file bytes, as 64 lowercase hexadecimal characters.
    pub sha256_hex: String,
}

/// The frozen compute choices of a job.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobCompute {
    /// Which computers render.
    pub target: ComputeChoice,
    /// What a remote worker sends back.
    pub transfer: TransferChoice,
    /// With a final-picture transfer: whether this computer renders a share too.
    #[serde(default)]
    pub contribute_local: bool,
}

/// Which computers render a job.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ComputeChoice {
    /// This computer only.
    Local,
    /// The remote worker only.
    Remote,
    /// This computer and the remote worker.
    Both,
}

impl ComputeChoice {
    /// The command-line word: `local`, `remote` or `both`.
    #[must_use]
    pub const fn cli_word(self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::Remote => "remote",
            Self::Both => "both",
        }
    }

    /// Reads a command-line word, ignoring ASCII case.
    #[must_use]
    pub fn from_cli_word(word: &str) -> Option<Self> {
        match word.trim().to_ascii_lowercase().as_str() {
            "local" => Some(Self::Local),
            "remote" => Some(Self::Remote),
            "both" => Some(Self::Both),
            _ => None,
        }
    }
}

/// What a remote worker sends back.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransferChoice {
    /// The raw sample data, merged on this computer.
    FullData,
    /// The finished picture only.
    FinalPicture,
}

impl TransferChoice {
    /// The command-line word: `full` or `final`.
    #[must_use]
    pub const fn cli_word(self) -> &'static str {
        match self {
            Self::FullData => "full",
            Self::FinalPicture => "final",
        }
    }

    /// Reads a command-line word, ignoring ASCII case.
    #[must_use]
    pub fn from_cli_word(word: &str) -> Option<Self> {
        match word.trim().to_ascii_lowercase().as_str() {
            "full" => Some(Self::FullData),
            "final" => Some(Self::FinalPicture),
            _ => None,
        }
    }
}

/// Which engines of this computer render. A machine setting, never stored in a job.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LocalEngines {
    /// The processor only.
    Cpu,
    /// The processor and the graphics card.
    #[default]
    CpuGpu,
    /// The graphics card only.
    Gpu,
}

impl LocalEngines {
    /// The command-line word: `cpu`, `cpu+gpu` or `gpu`.
    #[must_use]
    pub const fn cli_word(self) -> &'static str {
        match self {
            Self::Cpu => "cpu",
            Self::CpuGpu => "cpu+gpu",
            Self::Gpu => "gpu",
        }
    }

    /// Reads a command-line word, ignoring ASCII case. `cpugpu` and `cpu-gpu` are
    /// accepted as aliases of `cpu+gpu`.
    #[must_use]
    pub fn from_cli_word(word: &str) -> Option<Self> {
        match word.trim().to_ascii_lowercase().as_str() {
            "cpu" => Some(Self::Cpu),
            "cpu+gpu" | "cpugpu" | "cpu-gpu" => Some(Self::CpuGpu),
            "gpu" => Some(Self::Gpu),
            _ => None,
        }
    }
}

/// The colour space of the written picture or video frames.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobColorSpace {
    /// Standard sRGB.
    Srgb,
    /// Display P3.
    DisplayP3,
    /// Rec. 2020.
    Rec2020,
}

impl JobColorSpace {
    /// The name shown to people.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Srgb => "sRGB",
            Self::DisplayP3 => "Display P3",
            Self::Rec2020 => "Rec.2020",
        }
    }
}

/// What a job renders.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum JobKind {
    /// One still picture.
    Still(StillJob),
    /// A tilt performance video.
    TiltVideo(TiltVideoJob),
}

/// The settings of a still picture job.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StillJob {
    /// Samples per pixel.
    pub samples_per_pixel: u32,
    /// The colour space of the PNG.
    pub color_space: JobColorSpace,
    /// The lighting preset this picture was added with, if any. Informational.
    #[serde(default)]
    pub preset_label: String,
}

/// The settings of a tilt video job.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TiltVideoJob {
    /// The tilt axis, as the index the tilt section uses.
    pub axis_index: u32,
    /// The first tilt angle, in degrees.
    pub start_deg: f64,
    /// The last tilt angle, in degrees.
    pub end_deg: f64,
    /// The angle between frames, in degrees.
    pub step_deg: f64,
    /// The number of frames.
    pub total_frames: u32,
    /// Video frames per second.
    pub fps: u32,
    /// Samples per pixel for each frame.
    pub samples_per_pixel: u32,
    /// The colour space of the frames.
    pub color_space: JobColorSpace,
    /// Which metric overlays are drawn.
    #[serde(default)]
    pub overlay: OverlaySelection,
    /// The frozen metric curves, present when an overlay is selected.
    #[serde(default)]
    pub curves: Option<MetricCurvesData>,
    /// Whether the frames stay on disk after the video is made.
    #[serde(default)]
    pub keep_frames: bool,
    /// The name of the video file, without folder or extension.
    pub video_name: String,
}

/// Which metric overlays a tilt video draws.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct OverlaySelection {
    /// The brilliance curve.
    pub brilliance: bool,
    /// The windowing curve.
    pub windowing: bool,
    /// The extinction curve.
    pub extinction: bool,
    /// The tilt brilliance read-out.
    pub tilt_brilliance: bool,
    /// The angle read-out.
    pub angle: bool,
}

impl OverlaySelection {
    /// How many overlays are selected.
    #[must_use]
    pub fn count(&self) -> usize {
        [
            self.brilliance,
            self.windowing,
            self.extinction,
            self.tilt_brilliance,
            self.angle,
        ]
        .into_iter()
        .filter(|on| *on)
        .count()
    }
}

/// The three metric curves a tilt video overlay reads, one value per degree.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MetricCurvesData {
    /// The brilliance curve.
    pub brilliance: Vec<f32>,
    /// The windowing curve.
    pub windowing: Vec<f32>,
    /// The extinction curve.
    pub extinction: Vec<f32>,
}

impl RenderJobFile {
    /// The word the database stores as the job kind: `still` or `tilt_video`.
    #[must_use]
    pub const fn kind_word(&self) -> &'static str {
        match self.kind {
            JobKind::Still(_) => "still",
            JobKind::TiltVideo(_) => "tilt_video",
        }
    }

    /// The number of frames the job renders: 1 for a still.
    #[must_use]
    pub const fn frames_total(&self) -> u32 {
        match &self.kind {
            JobKind::Still(_) => 1,
            JobKind::TiltVideo(video) => video.total_frames,
        }
    }

    /// Resolves a path of the job: an absolute path stays, a relative one is joined to
    /// `base_dir`.
    #[must_use]
    pub fn resolve(&self, path: &str, base_dir: &Path) -> PathBuf {
        let path = Path::new(path);
        if path.is_absolute() {
            path.to_path_buf()
        } else {
            base_dir.join(path)
        }
    }

    /// The resolved output path (a PNG, or a frame folder).
    #[must_use]
    pub fn output_path(&self, base_dir: &Path) -> PathBuf {
        self.resolve(&self.output, base_dir)
    }

    /// The resolved HDR map path, when the job has one.
    #[must_use]
    pub fn hdr_path(&self, base_dir: &Path) -> Option<PathBuf> {
        self.hdr
            .as_ref()
            .map(|hdr| self.resolve(&hdr.path, base_dir))
    }

    /// Checks every rule a job must satisfy before it can be saved or rendered.
    ///
    /// # Errors
    ///
    /// [`JobFileError::Invalid`] with one plain sentence naming the first broken rule.
    pub fn validate(&self) -> Result<(), JobFileError> {
        self.validate_common()?;
        match &self.kind {
            JobKind::Still(still) => {
                check_spp(still.samples_per_pixel)?;
            }
            JobKind::TiltVideo(video) => validate_video(video)?,
        }
        self.validate_hdr()
    }

    fn validate_common(&self) -> Result<(), JobFileError> {
        if self.format != JOB_FORMAT_VERSION {
            return Err(invalid(format!(
                "This job file has format {}, but this version reads format {JOB_FORMAT_VERSION}.",
                self.format
            )));
        }
        if !is_token(&self.token) {
            return Err(invalid(
                "The job token must be 32 lowercase hexadecimal characters.",
            ));
        }
        if self.output.trim().is_empty() {
            return Err(invalid("The job has no output location."));
        }
        let side = MIN_DIM..=MAX_DIM;
        if !side.contains(&self.scene.width) || !side.contains(&self.scene.height) {
            return Err(invalid(format!(
                "The picture size must be between {MIN_DIM} and {MAX_DIM} pixels."
            )));
        }
        if !(1..=MAX_BOUNCES).contains(&self.scene.max_bounces) {
            return Err(invalid(format!(
                "The bounce limit must be between 1 and {MAX_BOUNCES}."
            )));
        }
        Ok(())
    }

    fn validate_hdr(&self) -> Result<(), JobFileError> {
        match (&self.hdr, self.scene.hdr()) {
            (None, None) => Ok(()),
            (Some(_), None) => Err(invalid(
                "The job names an HDR map file, but its scene is not lit by an HDR map.",
            )),
            (None, Some(_)) => Err(invalid(
                "The scene is lit by an HDR map, but the job does not name the map file.",
            )),
            (Some(source), Some(env)) => {
                if source.sha256_hex == indicatrix_net::messages::hash_hex(&env.content_hash) {
                    Ok(())
                } else {
                    Err(invalid(
                        "The HDR map hash in the job does not match the hash in its scene.",
                    ))
                }
            }
        }
    }
}

fn invalid(message: impl Into<String>) -> JobFileError {
    JobFileError::Invalid(message.into())
}

fn is_token(token: &str) -> bool {
    token.len() == 32
        && token
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn check_spp(spp: u32) -> Result<(), JobFileError> {
    if (1..=MAX_SPP).contains(&spp) {
        Ok(())
    } else {
        Err(invalid(format!(
            "The samples per pixel must be between 1 and {MAX_SPP}."
        )))
    }
}

fn validate_video(video: &TiltVideoJob) -> Result<(), JobFileError> {
    check_spp(video.samples_per_pixel)?;
    if !(1..=MAX_FPS).contains(&video.fps) {
        return Err(invalid(format!(
            "The frame rate must be between 1 and {MAX_FPS} frames per second."
        )));
    }
    if !(1..=MAX_FRAMES).contains(&video.total_frames) {
        return Err(invalid(format!(
            "A tilt video must have between 1 and {MAX_FRAMES} frames."
        )));
    }
    if !video.start_deg.is_finite() || !video.end_deg.is_finite() {
        return Err(invalid("The tilt start and end angles must be numbers."));
    }
    if !(video.step_deg.is_finite() && video.step_deg > 0.0) {
        return Err(invalid("The tilt step must be a number above zero."));
    }
    if let Some(curves) = &video.curves {
        let short = [&curves.brilliance, &curves.windowing, &curves.extinction]
            .into_iter()
            .any(|curve| curve.len() < CURVE_POINTS);
        if short {
            return Err(invalid(format!(
                "Each metric curve must hold at least {CURVE_POINTS} values."
            )));
        }
    }
    if video.overlay.count() > 0 && video.curves.is_none() {
        return Err(invalid(
            "The video overlay needs the metric curves, but the job holds none.",
        ));
    }
    Ok(())
}

/// A new random job token: 32 lowercase hexadecimal characters.
///
/// Built from two per-process random hasher keys mixed with the clock, so no random
/// number crate is needed.
#[must_use]
pub fn new_job_token() -> String {
    use std::hash::{BuildHasher, Hasher};
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos());
    let mut first = std::collections::hash_map::RandomState::new().build_hasher();
    first.write_u128(nanos);
    let mut second = std::collections::hash_map::RandomState::new().build_hasher();
    second.write_u128(nanos.rotate_left(17) ^ 0x9e37_79b9_7f4a_7c15);
    format!("{:016x}{:016x}", first.finish(), second.finish())
}

/// Test scenes and jobs shared by the tests of this crate.
#[cfg(test)]
pub(crate) mod test_support {
    use super::*;
    use indicatrix::{
        geometry::cuts::StandardGemCuts,
        optics::{materials::GemMaterial, raytracer::LightingPreset},
    };
    use indicatrix_net::scene::{HdrEnvironment, SceneEnvironment};

    pub const TOKEN: &str = "9f2c4a7d1e0b48c3a65d2f71c8e4b903";

    pub fn scene() -> SceneState {
        SceneState {
            width: 64,
            height: 48,
            yaw: 0.4,
            pitch: 0.3,
            distance: 3.0,
            light_yaw: 0.85,
            light_pitch: 0.95,
            exposure: 1.0,
            max_bounces: 4,
            lighting_preset: LightingPreset::Daylight,
            material: GemMaterial::diamond(),
            planes: StandardGemCuts::standard_round_brilliant(),
            girdle_frosted: false,
            backdrop: 0.0,
            environment: SceneEnvironment::Studio,
            surface_glare: 1.0,
            tools: Vec::new(),
            fluorescence: indicatrix::optics::fluorescence::Fluorescence::default(),
            head_shadow_deg: 22.0,
        }
    }

    pub fn hdr_scene() -> (SceneState, HdrSource) {
        let mut scene = scene();
        let hash = [7_u8; 32];
        scene.environment = SceneEnvironment::Hdr(HdrEnvironment {
            content_hash: hash,
            width: 64,
            height: 32,
        });
        let source = HdrSource {
            path: "assets/studio.hdr".to_string(),
            sha256_hex: indicatrix_net::messages::hash_hex(&hash),
        };
        (scene, source)
    }

    pub fn still_job() -> RenderJobFile {
        RenderJobFile {
            format: JOB_FORMAT_VERSION,
            token: TOKEN.to_string(),
            label: "Round · Current view · 64×48".to_string(),
            created_at: 1_791_302_580,
            design: DesignInfo {
                title: "Round".to_string(),
                designer: "J. Doe".to_string(),
                shape: "Round".to_string(),
                ri: "2.42".to_string(),
                material: "Diamond".to_string(),
            },
            scene: scene(),
            hdr: None,
            compute: JobCompute {
                target: ComputeChoice::Both,
                transfer: TransferChoice::FullData,
                contribute_local: false,
            },
            output: "C:\\renders\\gem.png".to_string(),
            kind: JobKind::Still(StillJob {
                samples_per_pixel: 64,
                color_space: JobColorSpace::Srgb,
                preset_label: String::new(),
            }),
        }
    }

    pub fn curves() -> MetricCurvesData {
        let curve = |scale: f32| {
            (0..CURVE_POINTS)
                .map(|index| (index as f32).mul_add(scale, 0.1))
                .collect::<Vec<f32>>()
        };
        MetricCurvesData {
            brilliance: curve(0.01),
            windowing: curve(0.02),
            extinction: curve(0.003),
        }
    }

    pub fn video_job() -> RenderJobFile {
        let (scene, source) = hdr_scene();
        RenderJobFile {
            scene,
            hdr: Some(source),
            output: "frames".to_string(),
            kind: JobKind::TiltVideo(TiltVideoJob {
                axis_index: 2,
                start_deg: -90.0,
                end_deg: 90.0,
                step_deg: 1.0,
                total_frames: 181,
                fps: 30,
                samples_per_pixel: 8,
                color_space: JobColorSpace::DisplayP3,
                overlay: OverlaySelection {
                    brilliance: true,
                    angle: true,
                    ..OverlaySelection::default()
                },
                curves: Some(curves()),
                keep_frames: true,
                video_name: "Round tilt".to_string(),
            }),
            ..still_job()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        test_support::{curves, hdr_scene, still_job, video_job},
        *,
    };

    fn message(job: &RenderJobFile) -> String {
        job.validate().unwrap_err().to_string()
    }

    fn video_mut(job: &mut RenderJobFile) -> &mut TiltVideoJob {
        match &mut job.kind {
            JobKind::TiltVideo(video) => video,
            JobKind::Still(_) => unreachable!("the test job is a video"),
        }
    }

    #[test]
    fn valid_jobs_pass() {
        assert_eq!(still_job().validate(), Ok(()));
        assert_eq!(video_job().validate(), Ok(()));
    }

    #[test]
    fn refuses_a_wrong_format() {
        let mut job = still_job();
        job.format = 2;
        assert!(message(&job).contains("format 2"));
    }

    #[test]
    fn refuses_a_bad_token() {
        for token in [
            "",
            "abc",
            "9F2C4A7D1E0B48C3A65D2F71C8E4B903",
            "9f2c4a7d1e0b48c3a65d2f71c8e4b90g",
        ] {
            let mut job = still_job();
            job.token = token.to_string();
            assert_eq!(
                message(&job),
                "The job token must be 32 lowercase hexadecimal characters."
            );
        }
    }

    #[test]
    fn refuses_an_empty_output() {
        let mut job = still_job();
        job.output = "  ".to_string();
        assert_eq!(message(&job), "The job has no output location.");
    }

    #[test]
    fn refuses_a_bad_size() {
        for (width, height) in [(15, 48), (64, 15), (8193, 48), (64, 8193)] {
            let mut job = still_job();
            job.scene.width = width;
            job.scene.height = height;
            assert_eq!(
                message(&job),
                "The picture size must be between 16 and 8192 pixels."
            );
        }
        let mut job = still_job();
        job.scene.width = 16;
        job.scene.height = 8192;
        assert_eq!(job.validate(), Ok(()));
    }

    #[test]
    fn refuses_bad_samples() {
        for spp in [0, MAX_SPP + 1] {
            let mut job = still_job();
            job.kind = JobKind::Still(StillJob {
                samples_per_pixel: spp,
                color_space: JobColorSpace::Srgb,
                preset_label: String::new(),
            });
            assert_eq!(
                message(&job),
                "The samples per pixel must be between 1 and 32768."
            );
            let mut video = video_job();
            video_mut(&mut video).samples_per_pixel = spp;
            assert!(message(&video).contains("samples per pixel"));
        }
    }

    #[test]
    fn refuses_bad_bounces() {
        for bounces in [0, MAX_BOUNCES + 1] {
            let mut job = still_job();
            job.scene.max_bounces = bounces;
            assert_eq!(message(&job), "The bounce limit must be between 1 and 128.");
        }
    }

    #[test]
    fn refuses_bad_video_numbers() {
        let mut job = video_job();
        video_mut(&mut job).fps = 0;
        assert!(message(&job).contains("frame rate"));
        video_mut(&mut job).fps = MAX_FPS + 1;
        assert!(message(&job).contains("frame rate"));

        let mut job = video_job();
        video_mut(&mut job).total_frames = 0;
        assert!(message(&job).contains("between 1 and 18001 frames"));
        video_mut(&mut job).total_frames = MAX_FRAMES + 1;
        assert!(message(&job).contains("between 1 and 18001 frames"));

        for step in [0.0, -1.0, f64::NAN, f64::INFINITY] {
            let mut job = video_job();
            video_mut(&mut job).step_deg = step;
            assert_eq!(message(&job), "The tilt step must be a number above zero.");
        }
        let mut job = video_job();
        video_mut(&mut job).end_deg = f64::NAN;
        assert!(message(&job).contains("start and end angles"));
    }

    #[test]
    fn refuses_short_curves_and_missing_curves() {
        let mut job = video_job();
        let mut short = curves();
        short.windowing.truncate(CURVE_POINTS - 1);
        video_mut(&mut job).curves = Some(short);
        assert!(message(&job).contains("at least 181 values"));

        let mut job = video_job();
        video_mut(&mut job).curves = None;
        assert!(message(&job).contains("needs the metric curves"));

        // No overlay and no curves is fine.
        video_mut(&mut job).overlay = OverlaySelection::default();
        assert_eq!(job.validate(), Ok(()));
    }

    #[test]
    fn refuses_hdr_mismatches() {
        let mut job = video_job();
        job.hdr = None;
        assert!(message(&job).contains("does not name the map file"));

        let mut job = still_job();
        job.hdr = video_job().hdr;
        assert!(message(&job).contains("not lit by an HDR map"));

        let mut job = video_job();
        if let Some(hdr) = job.hdr.as_mut() {
            hdr.sha256_hex = "00".repeat(32);
        }
        assert!(message(&job).contains("does not match"));

        let (scene, source) = hdr_scene();
        let mut job = still_job();
        job.scene = scene;
        job.hdr = Some(source);
        assert_eq!(job.validate(), Ok(()));
    }

    #[test]
    fn kind_words_and_frame_counts() {
        assert_eq!(still_job().kind_word(), "still");
        assert_eq!(still_job().frames_total(), 1);
        assert_eq!(video_job().kind_word(), "tilt_video");
        assert_eq!(video_job().frames_total(), 181);
    }

    #[test]
    fn resolves_relative_paths_against_the_base() {
        let job = video_job();
        let base = Path::new("base");
        assert_eq!(job.output_path(base), base.join("frames"));
        assert_eq!(
            job.hdr_path(base),
            Some(base.join("assets").join("studio.hdr"))
        );
        assert_eq!(still_job().hdr_path(base), None);
        let absolute = std::env::temp_dir().join("x.png");
        let text = absolute.to_string_lossy().into_owned();
        assert_eq!(job.resolve(&text, base), absolute);
    }

    #[test]
    fn cli_words_round_trip() {
        for choice in [
            ComputeChoice::Local,
            ComputeChoice::Remote,
            ComputeChoice::Both,
        ] {
            assert_eq!(
                ComputeChoice::from_cli_word(choice.cli_word()),
                Some(choice)
            );
        }
        for choice in [TransferChoice::FullData, TransferChoice::FinalPicture] {
            assert_eq!(
                TransferChoice::from_cli_word(choice.cli_word()),
                Some(choice)
            );
        }
        for engines in [LocalEngines::Cpu, LocalEngines::CpuGpu, LocalEngines::Gpu] {
            assert_eq!(
                LocalEngines::from_cli_word(engines.cli_word()),
                Some(engines)
            );
        }
        assert_eq!(
            LocalEngines::from_cli_word("CPUGPU"),
            Some(LocalEngines::CpuGpu)
        );
        assert_eq!(
            LocalEngines::from_cli_word("cpu-gpu"),
            Some(LocalEngines::CpuGpu)
        );
        assert_eq!(LocalEngines::from_cli_word("tpu"), None);
        assert_eq!(ComputeChoice::from_cli_word("all"), None);
        assert_eq!(TransferChoice::from_cli_word("raw"), None);
        assert_eq!(LocalEngines::default(), LocalEngines::CpuGpu);
    }

    #[test]
    fn labels_and_overlay_count() {
        assert_eq!(JobColorSpace::Srgb.label(), "sRGB");
        assert_eq!(JobColorSpace::DisplayP3.label(), "Display P3");
        assert_eq!(JobColorSpace::Rec2020.label(), "Rec.2020");
        assert_eq!(OverlaySelection::default().count(), 0);
        let all = OverlaySelection {
            brilliance: true,
            windowing: true,
            extinction: true,
            tilt_brilliance: true,
            angle: true,
        };
        assert_eq!(all.count(), 5);
    }

    #[test]
    fn tokens_have_the_right_shape_and_differ() {
        let first = new_job_token();
        let second = new_job_token();
        assert!(is_token(&first), "{first}");
        assert!(is_token(&second), "{second}");
        assert_ne!(first, second);
    }
}
