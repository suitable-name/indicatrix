//! Machine-readable record of one harness run.
//!
//! The report helpers in `common` and the phase modules feed every ULP comparison and
//! every Tier 3 image comparison into a process-global recorder as they print it, and
//! `main` adds one pass/fail entry per check group. When `--json <path>` was given,
//! [`write_json`] serialises the lot: the run date passed on the command line (the
//! harness never reads the clock), the adapter label, `indicatrix::BUILD_ID`, the maximum
//! genuine ULP per tier, every ULP check's statistics, every image comparison's z-score
//! statistics and the overall verdict. The owner commits that file as
//! `docs/gpu_harness_last_run.json` with each physics change (see `docs/gpu.md`).
//!
//! The JSON is written by hand: the example deliberately takes no serialisation
//! dependency (the crate's near-zero-dependency policy).

use std::{
    path::{Path, PathBuf},
    sync::{Mutex, PoisonError},
};

use indicatrix::renderer::gpu::estimator_check::ImageComparisonResult;

/// Usage text printed after a command-line error.
pub const USAGE: &str = "usage: gpu_equivalence_harness [--json <path> --date <YYYY-MM-DD>]\n  \
     --json <path>   write a machine-readable run summary to <path>\n  \
     --date <date>   run date recorded in that summary (required with --json; the harness \
     never reads the clock)";

/// Parsed command line.
#[derive(Debug, Default)]
pub struct CliOptions {
    /// Where to write the JSON summary, if requested.
    pub json_path: Option<PathBuf>,
    /// The run date recorded in the summary (`YYYY-MM-DD`); present exactly when
    /// `json_path` is.
    pub date: Option<String>,
}

/// Parses the harness's command line (without the program name).
///
/// `--json <path>` and `--date <YYYY-MM-DD>` must be given together or not at all, so a
/// committed summary always carries the date of the run it records.
///
/// # Errors
///
/// Returns a one-line message for an unknown argument, a missing value, a malformed date,
/// or an unpaired `--json` / `--date`.
pub fn parse_cli(args: impl IntoIterator<Item = String>) -> Result<CliOptions, String> {
    let mut options = CliOptions::default();
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--json" => {
                let path = args.next().ok_or("--json needs a path")?;
                options.json_path = Some(PathBuf::from(path));
            }
            "--date" => {
                let date = args.next().ok_or("--date needs a YYYY-MM-DD value")?;
                if !is_iso_date(&date) {
                    return Err(format!("--date must look like YYYY-MM-DD, got `{date}`"));
                }
                options.date = Some(date);
            }
            other => return Err(format!("unknown argument `{other}`")),
        }
    }
    if options.json_path.is_some() && options.date.is_none() {
        return Err("--json needs --date <YYYY-MM-DD>".to_owned());
    }
    if options.json_path.is_none() && options.date.is_some() {
        return Err("--date only applies together with --json".to_owned());
    }
    Ok(options)
}

/// Whether `text` has the shape `YYYY-MM-DD` (digits and two dashes; not a calendar check).
fn is_iso_date(text: &str) -> bool {
    let bytes = text.as_bytes();
    bytes.len() == 10
        && bytes.iter().enumerate().all(|(index, byte)| {
            if index == 4 || index == 7 {
                *byte == b'-'
            } else {
                byte.is_ascii_digit()
            }
        })
}

/// Which family of ULP comparisons a record belongs to; the summary reports the maximum
/// genuine ULP per family.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UlpTier {
    /// Per-function ULP budgets against the `optics::*` CPU functions (Tier 2).
    Tier2,
    /// The furnace anchor's per-tuple CPU-versus-GPU agreement.
    Furnace,
}

impl UlpTier {
    /// Every family, in the order the summary lists them.
    const ALL: [Self; 2] = [Self::Tier2, Self::Furnace];

    /// The key the family is reported under.
    const fn label(self) -> &'static str {
        match self {
            Self::Tier2 => "tier2_per_function",
            Self::Furnace => "furnace_per_tuple",
        }
    }
}

/// One ULP comparison as the report helpers see it.
///
/// "Genuine" ULP is the maximum over comparisons that were not exempted by the check's
/// absolute-difference floor; "raw" ULP includes the exempted ones.
#[derive(Clone, Copy, Debug)]
pub struct UlpComparison<'a> {
    /// The family this comparison belongs to.
    pub tier: UlpTier,
    /// The check's printed label.
    pub label: &'a str,
    /// How many values were compared.
    pub comparisons: usize,
    /// Maximum genuine ULP distance.
    pub max_genuine_ulp: u32,
    /// Maximum ULP distance including exempted near-zero comparisons.
    pub max_raw_ulp: u32,
    /// How many comparisons the absolute floor exempted.
    pub exempted: usize,
    /// The check's ULP budget, when the check reports one.
    pub budget: Option<u32>,
    /// Whether the check passed.
    pub passed: bool,
}

/// An owned [`UlpComparison`].
struct UlpRecord {
    tier: UlpTier,
    label: String,
    comparisons: u64,
    max_genuine_ulp: u64,
    max_raw_ulp: u64,
    exempted: u64,
    budget: Option<u64>,
    passed: bool,
}

/// One Tier 3 image comparison.
struct ImageRecord {
    label: String,
    width: u32,
    height: u32,
    total_pixels: u64,
    cpu_samples_per_pixel: u32,
    gpu_samples_per_pixel: u32,
    mean_z: f64,
    over_3_sigma_count: u64,
    over_3_sigma_expected_fraction: f64,
    max_abs_z: f64,
    largest_cluster: u64,
    passed: bool,
}

/// One check group's verdict.
struct GroupRecord {
    name: &'static str,
    passed: bool,
}

/// Everything recorded so far in this process.
struct Recorder {
    ulp: Vec<UlpRecord>,
    images: Vec<ImageRecord>,
    groups: Vec<GroupRecord>,
}

impl Recorder {
    const fn new() -> Self {
        Self {
            ulp: Vec::new(),
            images: Vec::new(),
            groups: Vec::new(),
        }
    }

    /// The `tiers` entry for one ULP family.
    fn tier_json(&self, tier: UlpTier) -> String {
        let records: Vec<&UlpRecord> = self.ulp.iter().filter(|r| r.tier == tier).collect();
        let max_genuine_ulp = records.iter().map(|r| r.max_genuine_ulp).max().unwrap_or(0);
        let max_raw_ulp = records.iter().map(|r| r.max_raw_ulp).max().unwrap_or(0);
        let within_budget = records.iter().all(|r| r.passed);
        format!(
            "    {}: {{ \"checks\": {}, \"max_genuine_ulp\": {max_genuine_ulp}, \
             \"max_raw_ulp\": {max_raw_ulp}, \"within_budget\": {within_budget} }}",
            json_string(tier.label()),
            records.len()
        )
    }

    /// The whole summary document.
    fn to_json(&self, header: &RunHeader<'_>, passed: bool) -> String {
        let groups: Vec<String> = self
            .groups
            .iter()
            .map(|g| {
                format!(
                    "    {{ \"name\": {}, \"passed\": {} }}",
                    json_string(g.name),
                    g.passed
                )
            })
            .collect();
        let tiers: Vec<String> = UlpTier::ALL.iter().map(|&t| self.tier_json(t)).collect();
        let ulp_checks: Vec<String> = self.ulp.iter().map(UlpRecord::to_json).collect();
        let images: Vec<String> = self.images.iter().map(ImageRecord::to_json).collect();
        format!(
            "{{\n  \"schema\": 1,\n  \"date\": {},\n  \"build_id\": {},\n  \"adapter\": {},\n  \
             \"passed\": {passed},\n  \"groups\": [\n{}\n  ],\n  \"tiers\": {{\n{}\n  }},\n  \
             \"ulp_checks\": [\n{}\n  ],\n  \"image_comparisons\": [\n{}\n  ]\n}}\n",
            json_string(header.date),
            json_string(header.build_id),
            json_string(header.adapter),
            groups.join(",\n"),
            tiers.join(",\n"),
            ulp_checks.join(",\n"),
            images.join(",\n"),
        )
    }
}

impl UlpRecord {
    fn to_json(&self) -> String {
        let budget = self
            .budget
            .map_or_else(|| "null".to_owned(), |b| b.to_string());
        format!(
            "    {{ \"tier\": {}, \"label\": {}, \"comparisons\": {}, \"max_genuine_ulp\": {}, \
             \"max_raw_ulp\": {}, \"exempted\": {}, \"budget\": {budget}, \"passed\": {} }}",
            json_string(self.tier.label()),
            json_string(&self.label),
            self.comparisons,
            self.max_genuine_ulp,
            self.max_raw_ulp,
            self.exempted,
            self.passed
        )
    }
}

impl ImageRecord {
    fn to_json(&self) -> String {
        format!(
            "    {{ \"label\": {}, \"width\": {}, \"height\": {}, \"total_pixels\": {}, \
             \"cpu_samples_per_pixel\": {}, \"gpu_samples_per_pixel\": {}, \"mean_z\": {}, \
             \"over_3_sigma_count\": {}, \"over_3_sigma_expected_fraction\": {}, \
             \"max_abs_z\": {}, \"largest_cluster\": {}, \"passed\": {} }}",
            json_string(&self.label),
            self.width,
            self.height,
            self.total_pixels,
            self.cpu_samples_per_pixel,
            self.gpu_samples_per_pixel,
            json_number(self.mean_z),
            self.over_3_sigma_count,
            json_number(self.over_3_sigma_expected_fraction),
            json_number(self.max_abs_z),
            self.largest_cluster,
            self.passed
        )
    }
}

/// The process-global recorder. The harness is single-threaded; the mutex only keeps the
/// static sound.
static RECORDER: Mutex<Recorder> = Mutex::new(Recorder::new());

/// Runs `f` on the recorder, tolerating a poisoned lock (a panicking check must not hide
/// what was recorded before it).
fn with_recorder<R>(f: impl FnOnce(&mut Recorder) -> R) -> R {
    f(&mut RECORDER.lock().unwrap_or_else(PoisonError::into_inner))
}

/// Records one ULP comparison.
pub fn record_ulp(summary: &UlpComparison<'_>) {
    let record = UlpRecord {
        tier: summary.tier,
        label: summary.label.to_owned(),
        comparisons: summary.comparisons as u64,
        max_genuine_ulp: u64::from(summary.max_genuine_ulp),
        max_raw_ulp: u64::from(summary.max_raw_ulp),
        exempted: summary.exempted as u64,
        budget: summary.budget.map(u64::from),
        passed: summary.passed,
    };
    with_recorder(|recorder| recorder.ulp.push(record));
}

/// Records one Tier 3 statistical image comparison.
pub fn record_image(label: &str, result: &ImageComparisonResult, passed: bool) {
    let record = ImageRecord {
        label: label.to_owned(),
        width: result.width,
        height: result.height,
        total_pixels: result.total_pixels as u64,
        cpu_samples_per_pixel: result.cpu_samples_per_pixel,
        gpu_samples_per_pixel: result.gpu_samples_per_pixel,
        mean_z: result.mean_z,
        over_3_sigma_count: result.over_3_sigma_count as u64,
        over_3_sigma_expected_fraction: result.over_3_sigma_expected,
        max_abs_z: result.max_abs_z,
        largest_cluster: result.cluster_sizes.first().copied().unwrap_or(0) as u64,
        passed,
    };
    with_recorder(|recorder| recorder.images.push(record));
}

/// Records one check group's overall verdict.
pub fn record_group(name: &'static str, passed: bool) {
    with_recorder(|recorder| recorder.groups.push(GroupRecord { name, passed }));
}

/// The identifying header of a run summary.
pub struct RunHeader<'a> {
    /// The run date, `YYYY-MM-DD`, exactly as passed on the command line.
    pub date: &'a str,
    /// `indicatrix::BUILD_ID` of the binary that ran.
    pub build_id: &'a str,
    /// The adapter's name, device type and backend.
    pub adapter: &'a str,
}

/// Writes the recorded run to `path` as JSON.
///
/// # Errors
///
/// Returns the I/O error from writing the file.
pub fn write_json(path: &Path, header: &RunHeader<'_>, passed: bool) -> std::io::Result<()> {
    let json = with_recorder(|recorder| recorder.to_json(header, passed));
    std::fs::write(path, json)
}

/// `text` as a JSON string literal. Control characters become spaces: none occurs in the
/// labels this harness produces, and dropping them keeps the output valid.
fn json_string(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if c.is_control() => out.push(' '),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// `value` as a JSON number, or `null` when it is NaN or infinite.
fn json_number(value: f64) -> String {
    if value.is_finite() {
        value.to_string()
    } else {
        "null".to_owned()
    }
}
