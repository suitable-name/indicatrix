//! The parsed command line: one typed argument struct per command, the [`Command`] that holds
//! them and the words the flags take.

use indicatrix::optics::LightingPreset;
use indicatrix_cut_core::ObjectivePreset;
use indicatrix_editor::retarget::CrownShift;
use indicatrix_render_jobs::{ComputeChoice, LocalEngines, TransferChoice};
use std::path::PathBuf;

/// The names `--lighting` takes, and the preset each stands for.
#[cfg(not(feature = "physical-color"))]
pub const LIGHTING_NAMES: [(&str, LightingPreset); 14] = [
    ("daylight", LightingPreset::Daylight),
    ("incandescent", LightingPreset::Incandescent),
    ("ring", LightingPreset::RingLights),
    ("spotlight", LightingPreset::DarkSpotlight),
    ("iso", LightingPreset::IsoHemisphere),
    // The grading tray is the preset's current name; `lighting_name` keeps answering "iso".
    ("grading", LightingPreset::IsoHemisphere),
    ("tent", LightingPreset::LightTent),
    ("dome", LightingPreset::DaylightDome),
    ("sun", LightingPreset::DaylightSun),
    ("tray", LightingPreset::WhiteTray),
    ("shop", LightingPreset::ShopLights),
    ("window", LightingPreset::WindowDaylight),
    ("illuminant-a", LightingPreset::IlluminantA),
    ("aset", LightingPreset::Aset),
];

/// The names `--lighting` takes, and the preset each stands for; the `physical-color` build adds
/// the two UV lamps.
#[cfg(feature = "physical-color")]
pub const LIGHTING_NAMES: [(&str, LightingPreset); 16] = [
    ("daylight", LightingPreset::Daylight),
    ("incandescent", LightingPreset::Incandescent),
    ("ring", LightingPreset::RingLights),
    ("spotlight", LightingPreset::DarkSpotlight),
    ("iso", LightingPreset::IsoHemisphere),
    // The grading tray is the preset's current name; `lighting_name` keeps answering "iso".
    ("grading", LightingPreset::IsoHemisphere),
    ("tent", LightingPreset::LightTent),
    ("dome", LightingPreset::DaylightDome),
    ("sun", LightingPreset::DaylightSun),
    ("tray", LightingPreset::WhiteTray),
    ("shop", LightingPreset::ShopLights),
    ("window", LightingPreset::WindowDaylight),
    ("illuminant-a", LightingPreset::IlluminantA),
    ("aset", LightingPreset::Aset),
    ("uv365", LightingPreset::UvLamp365),
    ("uv395", LightingPreset::UvLamp395),
];

/// The names `--preset` takes, and the objective each stands for.
pub const PRESET_NAMES: [(&str, ObjectivePreset); 7] = [
    ("balanced", ObjectivePreset::Balanced),
    ("brilliance", ObjectivePreset::Brilliance),
    ("low-windowing", ObjectivePreset::LowWindowing),
    ("low-extinction", ObjectivePreset::LowExtinction),
    ("keep-weight", ObjectivePreset::KeepWeight),
    ("lighten-dark", ObjectivePreset::LightenDark),
    ("intensify-pale", ObjectivePreset::IntensifyPale),
];

/// The word `--lighting` takes for `preset`.
#[must_use]
pub fn lighting_name(preset: LightingPreset) -> &'static str {
    LIGHTING_NAMES
        .iter()
        .find(|(_, known)| *known == preset)
        .map_or("custom", |(name, _)| name)
}

/// The word `--preset` takes for `preset`.
#[must_use]
pub fn preset_name(preset: ObjectivePreset) -> &'static str {
    PRESET_NAMES
        .iter()
        .find(|(_, known)| *known == preset)
        .map_or("custom", |(name, _)| name)
}

/// Which help page to print.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Topic {
    /// The command list.
    Root,
    /// `info`.
    Info,
    /// `solve`.
    Solve,
    /// `metrics`.
    Metrics,
    /// `validate`.
    Validate,
    /// `optimize`.
    Optimize,
    /// `retarget`.
    Retarget,
    /// `sweep`.
    Sweep,
    /// `export`.
    Export,
    /// `render`.
    Render,
    /// `tilt-video`.
    TiltVideo,
}

impl Topic {
    /// The topic a command word names; [`Topic::Root`] for anything else.
    #[must_use]
    pub fn of(word: &str) -> Self {
        match word {
            "info" => Self::Info,
            "solve" => Self::Solve,
            "metrics" => Self::Metrics,
            "validate" => Self::Validate,
            "optimize" => Self::Optimize,
            "retarget" => Self::Retarget,
            "sweep" => Self::Sweep,
            "export" => Self::Export,
            "render" => Self::Render,
            "tilt-video" => Self::TiltVideo,
            _ => Self::Root,
        }
    }
}

/// A material named on the command line.
#[derive(Debug, Clone, PartialEq)]
pub enum MaterialArg {
    /// `--material NAME`: a built-in preset or a custom material of the `--db` library.
    Named(String),
    /// `--ri N`: a bare refractive index, a flat (non-dispersive) material.
    Ri(f64),
}

/// How a report is printed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReportFormat {
    /// Aligned text for a person.
    Text,
    /// JSON with sorted keys.
    Json,
    /// One header line and one row of values.
    Csv,
}

/// Which algorithm `retarget` runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// The critical-angle shift (the dialog's default).
    Shift,
    /// The shift, then a search around it.
    Optimize,
}

/// What `export` writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportFormat {
    /// `GemCAD` cutting instructions, the file the editor's Export writes: CRLF line ends
    /// (the library-entry footnote the desktop adds is left out).
    Asc,
    /// A self-contained `.indicatrix` design file.
    Indicatrix,
    /// A Gem Cut Studio file (experimental).
    Gcs,
    /// The cutting sheet as a web page.
    Html,
}

impl ExportFormat {
    /// The format a file name's extension names, if it names one.
    #[must_use]
    pub fn from_file_name(name: &str) -> Option<Self> {
        let lower = name.to_ascii_lowercase();
        let extension = lower.rsplit_once('.').map(|(_, extension)| extension)?;
        match extension {
            "asc" => Some(Self::Asc),
            "indicatrix" => Some(Self::Indicatrix),
            "gcs" => Some(Self::Gcs),
            "html" | "htm" => Some(Self::Html),
            _ => None,
        }
    }
}

/// `info`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InfoArgs {
    /// The design file.
    pub design: PathBuf,
    /// Print JSON instead of text.
    pub json: bool,
    /// Write the report here instead of printing it.
    pub out: Option<PathBuf>,
    /// A design library, for its custom materials.
    pub db: Option<PathBuf>,
}

/// `solve`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SolveArgs {
    /// The design file.
    pub design: PathBuf,
    /// Print JSON instead of text.
    pub json: bool,
    /// Save the design here as a `.indicatrix` file.
    pub out: Option<PathBuf>,
    /// A design library, for its custom materials.
    pub db: Option<PathBuf>,
}

/// `metrics`.
#[derive(Debug, Clone, PartialEq)]
pub struct MetricsArgs {
    /// The design file.
    pub design: PathBuf,
    /// Score in this material instead of the design's own.
    pub material: Option<MaterialArg>,
    /// Also average the tilt performance (about a second and a half).
    pub tilt: bool,
    /// How to print.
    pub format: ReportFormat,
    /// The light the score is taken under.
    pub lighting: LightingPreset,
    /// Write the report here instead of printing it.
    pub out: Option<PathBuf>,
    /// A design library, for its custom materials.
    pub db: Option<PathBuf>,
}

/// `validate`.
#[derive(Debug, Clone, PartialEq)]
pub struct ValidateArgs {
    /// The design file.
    pub design: PathBuf,
    /// Print JSON instead of text.
    pub json: bool,
    /// Judge the optics in this material instead of the design's own.
    pub material: Option<MaterialArg>,
    /// The light the optics are measured under.
    pub lighting: LightingPreset,
    /// Write the report here instead of printing it.
    pub out: Option<PathBuf>,
    /// A design library, for its custom materials.
    pub db: Option<PathBuf>,
}

/// `optimize`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OptimizeArgs {
    /// The design file.
    pub design: PathBuf,
    /// What the search favours.
    pub preset: ObjectivePreset,
    /// Evaluations of the coordinate stage.
    pub budget: usize,
    /// How many starting arrangements the search tries, 1 to 32.
    pub starts: usize,
    /// The search seed.
    pub seed: u64,
    /// Turn pinned (scale-reference) tiers about their girdle edges too.
    pub vary_anchored: bool,
    /// How many ranked candidates to keep.
    pub candidates: usize,
    /// The light the search scores under.
    pub lighting: LightingPreset,
    /// Print JSON instead of text.
    pub json: bool,
    /// Save the best candidate here as a `.indicatrix` file.
    pub out: Option<PathBuf>,
    /// A design library, for its custom materials.
    pub db: Option<PathBuf>,
}

/// `retarget`.
#[derive(Debug, Clone, PartialEq)]
pub struct RetargetArgs {
    /// The design file.
    pub design: PathBuf,
    /// The material to retarget for.
    pub target: MaterialArg,
    /// Shift alone, or shift and search.
    pub mode: Mode,
    /// How far the crown follows the pavilion's shift.
    pub crown: CrownShift,
    /// What the search favours (`--mode optimize`).
    pub preset: ObjectivePreset,
    /// Degrees either side of each angle the search may move; the dialog's default when absent.
    pub range_deg: Option<f64>,
    /// Evaluations of the search; the dialog's default when absent.
    pub budget: Option<usize>,
    /// The search seed.
    pub seed: u64,
    /// Penalise options that drift from the design's table size and crown-to-pavilion ratio
    /// (`--mode optimize`; `--no-keep-look` turns it off).
    pub keep_look: bool,
    /// The light everything is scored under.
    pub lighting: LightingPreset,
    /// Print JSON instead of text.
    pub json: bool,
    /// Save the retargeted design here as a `.indicatrix` file.
    pub out: Option<PathBuf>,
    /// A design library, for its custom materials.
    pub db: Option<PathBuf>,
}

/// `sweep`.
#[derive(Debug, Clone, PartialEq)]
pub struct SweepArgs {
    /// The design file.
    pub design: PathBuf,
    /// The tier to sweep: its name, or `#N` for the Nth row.
    pub tier: String,
    /// One end of the range, in degrees from flat (a sign in front is ignored: the side of
    /// the girdle comes from the tier).
    pub from_deg: f64,
    /// The other end.
    pub to_deg: f64,
    /// The distance between angles.
    pub step_deg: f64,
    /// Also average the tilt performance of every row.
    pub tilt: bool,
    /// Write the rows here as CSV.
    pub csv: Option<PathBuf>,
    /// Print JSON instead of text.
    pub json: bool,
    /// Score in this material instead of the design's own.
    pub material: Option<MaterialArg>,
    /// The light the score is taken under.
    pub lighting: LightingPreset,
    /// A design library, for its custom materials.
    pub db: Option<PathBuf>,
}

/// `export`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportArgs {
    /// The design file.
    pub design: PathBuf,
    /// What to write.
    pub format: ExportFormat,
    /// Where to write it.
    pub out: PathBuf,
    /// A design library, for its custom materials.
    pub db: Option<PathBuf>,
    /// The date the html cutting sheet prints under its title (`October 2026`); `None` prints
    /// no date, which keeps the output the same every time.
    pub date: Option<String>,
}

/// Which of the two render commands a [`RenderJobArgs`] belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobCommandKind {
    /// `render`: a still picture.
    Render,
    /// `tilt-video`: a tilt performance video.
    TiltVideo,
}

impl JobCommandKind {
    /// The command word, which is also the word progress lines start with.
    #[must_use]
    pub const fn word(self) -> &'static str {
        match self {
            Self::Render => "render",
            Self::TiltVideo => "tilt-video",
        }
    }
}

/// `render` and `tilt-video`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderJobArgs {
    /// Which command this is.
    pub kind: JobCommandKind,
    /// The job file.
    pub job: PathBuf,
    /// `--out` (`render`: the PNG to write) or `--out-dir` (`tilt-video`: the frame folder),
    /// instead of the job's own.
    pub out: Option<PathBuf>,
    /// The engines of this computer that render.
    pub local: LocalEngines,
    /// Replaces the job's compute choice.
    pub compute: Option<ComputeChoice>,
    /// Replaces the job's transfer choice.
    pub transfer: Option<TransferChoice>,
    /// The remote worker's `HOST:PORT`.
    pub remote: Option<String>,
    /// The folder with the remote worker's certificates.
    pub cert_dir: Option<PathBuf>,
    /// With a final-picture transfer: this computer renders a share too.
    pub contribute_local: bool,
    /// `tilt-video` only: start again from the first frame.
    pub restart: bool,
    /// No progress lines.
    pub quiet: bool,
}

/// A parsed command line.
#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    /// Print a help page.
    Help(Topic),
    /// Print the version.
    Version,
    /// `info`.
    Info(InfoArgs),
    /// `solve`.
    Solve(SolveArgs),
    /// `metrics`.
    Metrics(MetricsArgs),
    /// `validate`.
    Validate(ValidateArgs),
    /// `optimize`.
    Optimize(OptimizeArgs),
    /// `retarget`.
    Retarget(RetargetArgs),
    /// `sweep`.
    Sweep(SweepArgs),
    /// `export`.
    Export(ExportArgs),
    /// `render`.
    Render(RenderJobArgs),
    /// `tilt-video`.
    TiltVideo(RenderJobArgs),
}
