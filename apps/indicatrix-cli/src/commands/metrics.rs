//! `metrics`: the optical and geometric figures of a design, as text, JSON or one CSV row.
//!
//! The optical figures are the ones the Optimize tab and the sweep score with: one table-up
//! measurement (about 2 ms) under the lighting preset at the canonical light pose, and with
//! `--tilt` the mean over four axes and 181 tilt angles (about a second and a half). The
//! proportions and the yield come from the solid. Concave tools are not part of the figures, as
//! in Optimize: the stone is scored on its flat facets.

use super::{deliver, key_values, list_block, open};
use crate::{
    args::{MetricsArgs, ReportFormat, lighting_name},
    format::{
        INDEX_DECIMALS, PERCENT_DECIMALS, csv_line, fixed, fixed_or_na, json_number, json_optional,
        json_text,
    },
    load::Loaded,
    materials::{Catalogue, Scoring, resolve_scoring},
    outcome::{CliError, CommandResult},
    stone::{Analysis, analyze},
};
use indicatrix::{
    color::metrics::{GemOpticalMetrics, evaluate_gem_optical_metrics},
    geometry::{
        meet_solver::SolvedTier,
        stone_metrics::{StoneProportions, measure_solid},
    },
    optics::LightingPreset,
};
use indicatrix_cut_core::{
    Design, ObjectiveComponents, ObjectiveFidelity, evaluate_objective_under,
    optimize::{CANONICAL_LIGHT_PITCH, CANONICAL_LIGHT_YAW},
    volumetric_yield,
};
use indicatrix_editor::{
    optimize_view::facet_count_from_solved, solve_policy::design_to_gpu_planes_from_solved,
};
use serde_json::{Map, Value, json};
use std::fmt::Write as _;

/// The heading of the table-up figures.
const GROUP_TABLE_UP: &str = "Optics, table up";

/// The heading of the tilt averages.
const GROUP_TILT: &str = "Optics, tilt average over 4 axes and 181 angles";

/// The heading of the proportions.
const GROUP_PROPORTIONS: &str = "Proportions";

/// The heading of the counts and the yield.
const GROUP_STONE: &str = "Stone";

/// The camera pitch of the table-up figures (90 degrees): looking straight down on the table,
/// as the sweep and Optimize's fast score do.
const TABLE_UP_PITCH: f32 = std::f32::consts::FRAC_PI_2;

/// What one figure reads as.
enum Reading {
    /// A whole number.
    Count(usize),
    /// A measurement with a fixed number of decimals; `None` when it cannot be read off this
    /// stone (a design with no girdle has no girdle thickness).
    Number { value: Option<f64>, decimals: usize },
}

/// One figure of the report.
struct Figure {
    /// The group it is printed under.
    group: &'static str,
    /// The JSON key and the CSV column.
    key: &'static str,
    /// The text label.
    label: &'static str,
    /// The unit printed after the value in text (`%`, or nothing).
    unit: &'static str,
    /// The value.
    reading: Reading,
}

impl Figure {
    const fn percent(
        group: &'static str,
        key: &'static str,
        label: &'static str,
        value: Option<f64>,
    ) -> Self {
        Self {
            group,
            key,
            label,
            unit: "%",
            reading: Reading::Number {
                value,
                decimals: PERCENT_DECIMALS,
            },
        }
    }

    const fn plain(
        group: &'static str,
        key: &'static str,
        label: &'static str,
        value: Option<f64>,
    ) -> Self {
        Self {
            unit: "",
            ..Self::percent(group, key, label, value)
        }
    }

    const fn count(
        group: &'static str,
        key: &'static str,
        label: &'static str,
        value: usize,
    ) -> Self {
        Self {
            group,
            key,
            label,
            unit: "",
            reading: Reading::Count(value),
        }
    }

    /// The value as text, without its unit.
    fn value_text(&self) -> String {
        match &self.reading {
            Reading::Count(count) => count.to_string(),
            Reading::Number { value, decimals } => fixed_or_na(*value, *decimals),
        }
    }

    /// The value and its unit, as the text report prints them.
    fn text(&self) -> String {
        if self.unit.is_empty() {
            self.value_text()
        } else {
            format!("{} {}", self.value_text(), self.unit)
        }
    }

    fn json(&self) -> Value {
        match &self.reading {
            Reading::Count(count) => Value::from(*count),
            Reading::Number { value, decimals } => {
                json_optional(*value, i32::try_from(*decimals).unwrap_or(2))
            }
        }
    }

    fn csv(&self) -> String {
        match &self.reading {
            Reading::Count(count) => count.to_string(),
            Reading::Number { value, decimals } => {
                value.map_or_else(String::new, |v| fixed(v, *decimals))
            }
        }
    }
}

/// Everything measured off one stone.
struct Measured {
    table_up: GemOpticalMetrics,
    tilt: Option<ObjectiveComponents>,
    proportions: StoneProportions,
    vol_w3: Option<f64>,
    depth_pct: Option<f64>,
    yield_pct: Option<f64>,
    facets: usize,
    tiers: usize,
    warnings: usize,
}

/// Measures the stone of `design`, whose solve is `solved` and whose analysis is `analysis`.
fn measure(
    design: &Design,
    analysis: &Analysis,
    solved: &[SolvedTier],
    scoring: &Scoring,
    lighting: LightingPreset,
    tilt: bool,
) -> Result<Measured, CliError> {
    let planes_gpu = design_to_gpu_planes_from_solved(design, solved);
    // Sized by the design's girdle diameter, like the render and `optimize`.
    let gem = &scoring.sized_gem(design, &planes_gpu);
    let environment = lighting.studio(1.0, CANONICAL_LIGHT_YAW, CANONICAL_LIGHT_PITCH);
    let table_up = evaluate_gem_optical_metrics(&planes_gpu, gem, 0.0, TABLE_UP_PITCH, environment);
    let tilt =
        tilt.then(|| evaluate_objective_under(&planes_gpu, gem, ObjectiveFidelity::Full, lighting));
    let planes = design.planes_from_solved(solved);
    let solid = measure_solid(&planes)
        .ok_or_else(|| CliError::design("the stone cannot be measured: it has no usable volume"))?;
    let mesh = analysis
        .mesh
        .as_ref()
        .ok_or_else(|| CliError::design("the stone has no solid to measure"))?;
    let width = solid.width_axis;
    let (vol_w3, depth_pct) = if width > 1e-9 {
        (
            Some(solid.volume / (width * width * width)),
            Some(100.0 * solid.total_height / width),
        )
    } else {
        (None, None)
    };
    let yield_pct = measure_solid(&design.preform.planes())
        .and_then(|rough| volumetric_yield(solid.volume, rough.volume))
        .map(|fraction| fraction * 100.0);
    Ok(Measured {
        table_up,
        tilt,
        proportions: StoneProportions::from_solid(&solid, mesh, &planes),
        vol_w3,
        depth_pct,
        yield_pct,
        facets: facet_count_from_solved(design, solved),
        tiers: design.tiers.len(),
        warnings: analysis.warnings.len(),
    })
}

/// The figures of `measured`, in report order.
fn figures(measured: &Measured) -> Vec<Figure> {
    let mut all = optics_figures(measured);
    all.extend(shape_figures(measured));
    all
}

/// The table-up figures, and the tilt averages when they were measured.
fn optics_figures(measured: &Measured) -> Vec<Figure> {
    let table = &measured.table_up;
    let mut figures = vec![
        Figure::percent(
            GROUP_TABLE_UP,
            "table_up_brilliance_pct",
            "Brilliance",
            Some(f64::from(table.brilliance_pct)),
        ),
        Figure::percent(
            GROUP_TABLE_UP,
            "table_up_windowing_pct",
            "Windowing",
            Some(f64::from(table.windowing_pct)),
        ),
        Figure::percent(
            GROUP_TABLE_UP,
            "table_up_extinction_pct",
            "Extinction",
            Some(f64::from(table.extinction_pct)),
        ),
        Figure::plain(
            GROUP_TABLE_UP,
            "table_up_fire_index",
            "Fire index",
            Some(f64::from(table.fire_index)),
        ),
        Figure::percent(
            GROUP_TABLE_UP,
            "table_up_scintillation_pct",
            "Scintillation",
            Some(f64::from(table.scintillation_pct)),
        ),
    ];
    if let Some(tilt) = &measured.tilt {
        figures.extend([
            Figure::percent(
                GROUP_TILT,
                "tilt_brilliance_pct",
                "Brilliance",
                Some(f64::from(tilt.tilt_brilliance_pct)),
            ),
            Figure::percent(
                GROUP_TILT,
                "tilt_windowing_pct",
                "Windowing",
                Some(f64::from(tilt.windowing_pct)),
            ),
            Figure::percent(
                GROUP_TILT,
                "tilt_extinction_pct",
                "Extinction",
                Some(f64::from(tilt.extinction_pct)),
            ),
        ]);
    }
    figures
}

/// The proportions, the yield and the counts.
fn shape_figures(measured: &Measured) -> Vec<Figure> {
    let proportions = &measured.proportions;
    vec![
        Figure::percent(
            GROUP_PROPORTIONS,
            "table_pct_of_width",
            "Table, of width",
            proportions.table_percent,
        ),
        Figure::percent(
            GROUP_PROPORTIONS,
            "crown_pct_of_width",
            "Crown height, of width",
            proportions.crown_to_width_percent,
        ),
        Figure::percent(
            GROUP_PROPORTIONS,
            "pavilion_pct_of_width",
            "Pavilion depth, of width",
            proportions.pavilion_to_width_percent,
        ),
        Figure::percent(
            GROUP_PROPORTIONS,
            "girdle_pct_of_width",
            "Girdle thickness, of width",
            proportions.girdle_to_width_percent,
        ),
        Figure::percent(
            GROUP_PROPORTIONS,
            "total_depth_pct_of_width",
            "Total depth, of width",
            measured.depth_pct,
        ),
        Figure::plain(
            GROUP_PROPORTIONS,
            "length_to_width",
            "Length to width",
            proportions.length_to_width,
        ),
        Figure::plain(
            GROUP_PROPORTIONS,
            "volume_over_width_cubed",
            "Volume over width cubed",
            measured.vol_w3,
        ),
        Figure::percent(
            GROUP_STONE,
            "yield_pct",
            "Yield, of the rough",
            measured.yield_pct,
        ),
        Figure::count(GROUP_STONE, "facets", "Facets", measured.facets),
        Figure::count(GROUP_STONE, "tiers", "Tiers", measured.tiers),
        Figure::count(GROUP_STONE, "warnings", "Warnings", measured.warnings),
    ]
}

/// A measured design, ready to print.
struct Built {
    name: String,
    file: String,
    material: String,
    n_d: f64,
    lighting: &'static str,
    figures: Vec<Figure>,
    notes: Vec<String>,
}

/// Measures `loaded` for `args`.
///
/// # Errors
///
/// [`CliError`] when the material is unknown or the design is not a usable stone.
fn build(loaded: &Loaded, catalogue: &Catalogue, args: &MetricsArgs) -> Result<Built, CliError> {
    let scoring = resolve_scoring(&loaded.design, catalogue, args.material.as_ref())?;
    let analysis = analyze(&loaded.design);
    let solved = analysis.require_stone(&loaded.design)?;
    let measured = measure(
        &loaded.design,
        &analysis,
        solved,
        &scoring,
        args.lighting,
        args.tilt,
    )?;
    let mut notes = loaded.notes.clone();
    notes.extend(scoring.note.clone());
    Ok(Built {
        name: loaded.name.clone(),
        file: loaded.file_name.clone(),
        material: scoring.label.clone(),
        n_d: scoring.n_d(),
        lighting: lighting_name(args.lighting),
        figures: figures(&measured),
        notes,
    })
}

/// The figures as groups of right-aligned values.
fn figure_lines(figures: &[Figure]) -> String {
    let label_width = figures
        .iter()
        .map(|figure| figure.label.chars().count())
        .max()
        .unwrap_or(0);
    let value_width = figures
        .iter()
        .map(|figure| figure.text().chars().count())
        .max()
        .unwrap_or(0);
    let mut text = String::new();
    let mut group = "";
    for figure in figures {
        if figure.group != group {
            if !text.is_empty() {
                text.push('\n');
            }
            text.push_str(figure.group);
            text.push_str(":\n");
            group = figure.group;
        }
        let _ = writeln!(
            text,
            "  {:<label_width$}  {:>value_width$}",
            figure.label,
            figure.text()
        );
    }
    text
}

/// The text report.
fn text_report(built: &Built) -> String {
    let mut text = key_values(&[
        ("Design", format!("{} ({})", built.name, built.file)),
        ("Material", built.material.clone()),
        ("Index", fixed(built.n_d, INDEX_DECIMALS)),
        ("Lighting", built.lighting.to_string()),
    ]);
    text.push('\n');
    text.push_str(&figure_lines(&built.figures));
    if !built.notes.is_empty() {
        text.push('\n');
        text.push_str(&list_block("Notes", &built.notes));
    }
    text
}

/// The JSON report.
fn json_report(built: &Built) -> String {
    let mut figures = Map::new();
    for figure in &built.figures {
        figures.insert(figure.key.to_string(), figure.json());
    }
    json_text(&json!({
        "design": built.name,
        "file": built.file,
        "material": built.material,
        "refractive_index": json_number(built.n_d, i32::try_from(INDEX_DECIMALS).unwrap_or(4)),
        "lighting": built.lighting,
        "figures": figures,
        "notes": built.notes,
    }))
}

/// The CSV report: a header line and one row. A figure the stone has no value for is left
/// empty. Notes are not part of it (they go to standard error).
fn csv_report(built: &Built) -> String {
    let mut header: Vec<String> = ["design", "material", "refractive_index", "lighting"]
        .iter()
        .map(ToString::to_string)
        .collect();
    let mut row = vec![
        built.name.clone(),
        built.material.clone(),
        fixed(built.n_d, INDEX_DECIMALS),
        built.lighting.to_string(),
    ];
    for figure in &built.figures {
        header.push(figure.key.to_string());
        row.push(figure.csv());
    }
    let mut text = csv_line(&header);
    text.push_str(&csv_line(&row));
    text
}

/// Runs `metrics`.
///
/// # Errors
///
/// [`CliError`] when the design cannot be opened, the material is unknown, or the design is not
/// a usable stone (exit 2).
pub fn run(args: &MetricsArgs) -> CommandResult {
    let (loaded, catalogue) = open(&args.design, args.db.as_deref())?;
    let built = build(&loaded, &catalogue, args)?;
    let report = match args.format {
        ReportFormat::Text => text_report(&built),
        ReportFormat::Json => json_report(&built),
        ReportFormat::Csv => csv_report(&built),
    };
    let mut outcome = deliver(report, args.out.as_deref());
    if matches!(args.format, ReportFormat::Csv) {
        for note in &built.notes {
            outcome = outcome.with_note(&format!("note: {note}"));
        }
    }
    Ok(outcome)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        args::MaterialArg,
        outcome::{EXIT_DESIGN, EXIT_OK, EXIT_USAGE},
        testing,
    };
    use indicatrix_cut_core::CANONICAL_LIGHTING_PRESET;
    use std::path::PathBuf;

    fn args(tilt: bool, format: ReportFormat) -> MetricsArgs {
        MetricsArgs {
            design: PathBuf::new(),
            material: Some(MaterialArg::Ri(1.76)),
            tilt,
            format,
            lighting: CANONICAL_LIGHTING_PRESET,
            out: None,
            db: None,
        }
    }

    fn built(tilt: bool) -> Built {
        let mut design = testing::template();
        design.meta.headers = vec!["Round".to_string()];
        let loaded = Loaded::in_memory(design, "round.indicatrix");
        build(
            &loaded,
            &Catalogue::default(),
            &args(tilt, ReportFormat::Text),
        )
        .expect("measures")
    }

    fn figure<'a>(built: &'a Built, key: &str) -> &'a Figure {
        built
            .figures
            .iter()
            .find(|figure| figure.key == key)
            .unwrap_or_else(|| panic!("no figure {key}"))
    }

    #[test]
    fn the_table_up_figures_are_percentages_of_the_light() {
        let built = built(false);
        for key in [
            "table_up_brilliance_pct",
            "table_up_windowing_pct",
            "table_up_extinction_pct",
        ] {
            let value: f64 = figure(&built, key).value_text().parse().expect("a number");
            assert!((0.0..=100.0).contains(&value), "{key} = {value}");
        }
        assert!(
            built
                .figures
                .iter()
                .all(|figure| figure.group != GROUP_TILT)
        );
        assert_eq!(built.material, "refractive index 1.7600");
        assert!((built.n_d - 1.76).abs() < 1e-9);
    }

    #[test]
    fn the_proportions_and_counts_describe_the_template() {
        let built = built(false);
        let tiers = figure(&built, "tiers").value_text();
        assert_eq!(tiers, testing::template().tiers.len().to_string());
        let facets: usize = figure(&built, "facets")
            .value_text()
            .parse()
            .expect("a count");
        assert!(facets > 8, "{facets}");
        let yield_pct: f64 = figure(&built, "yield_pct")
            .value_text()
            .parse()
            .expect("a number");
        assert!(yield_pct > 0.0 && yield_pct <= 100.0, "{yield_pct}");
        let table: f64 = figure(&built, "table_pct_of_width")
            .value_text()
            .parse()
            .expect("a round brilliant has a table");
        assert!(table > 30.0 && table < 90.0, "{table}");
    }

    #[test]
    fn the_text_report_groups_the_figures() {
        let text = text_report(&built(false));
        assert!(text.starts_with("Design:    "), "{text}");
        assert!(text.contains("\nOptics, table up:\n  Brilliance"), "{text}");
        assert!(text.contains("\nProportions:\n"), "{text}");
        assert!(text.contains("\nStone:\n"), "{text}");
        assert!(
            text.contains("Material:  refractive index 1.7600\n"),
            "{text}"
        );
        assert!(!text.contains("tilt average"), "{text}");
        assert!(text.ends_with('\n'));
    }

    #[test]
    fn the_json_report_has_one_number_per_figure() {
        let built = built(false);
        let value: Value = serde_json::from_str(&json_report(&built)).expect("valid JSON");
        let figures = value["figures"].as_object().expect("a figures object");
        assert_eq!(figures.len(), built.figures.len());
        assert!(figures["table_up_brilliance_pct"].is_number());
        assert!(figures["facets"].is_u64());
        assert_eq!(value["lighting"], lighting_name(CANONICAL_LIGHTING_PRESET));
        assert_eq!(value["refractive_index"], 1.76);
        assert!(figures.get("tilt_brilliance_pct").is_none());
    }

    #[test]
    fn the_csv_report_is_a_header_and_one_row_of_the_same_width() {
        let text = csv_report(&built(false));
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 2, "{text}");
        let header: Vec<&str> = lines[0].split(',').collect();
        let row: Vec<&str> = lines[1].split(',').collect();
        assert_eq!(header.len(), row.len());
        assert_eq!(
            &header[..4],
            ["design", "material", "refractive_index", "lighting"]
        );
        assert_eq!(row[2], "1.7600");
        assert_eq!(row[3], lighting_name(CANONICAL_LIGHTING_PRESET));
        assert!(header.contains(&"table_up_brilliance_pct"));
    }

    #[test]
    fn the_report_is_the_same_every_time() {
        assert_eq!(text_report(&built(false)), text_report(&built(false)));
        assert_eq!(json_report(&built(false)), json_report(&built(false)));
    }

    #[test]
    fn a_figure_with_no_value_reads_n_a_in_text_null_in_json_and_empty_in_csv() {
        let missing = Figure::percent(GROUP_PROPORTIONS, "girdle_pct_of_width", "Girdle", None);
        assert_eq!(missing.text(), "n/a %");
        assert_eq!(missing.json(), Value::Null);
        assert_eq!(missing.csv(), "");
        let count = Figure::count(GROUP_STONE, "facets", "Facets", 12);
        assert_eq!(count.text(), "12");
        assert_eq!(count.json(), Value::from(12_usize));
        assert_eq!(count.csv(), "12");
    }

    #[test]
    fn end_to_end_metrics_prints_json() {
        let input = testing::write_design("metrics-e2e.indicatrix", &testing::template());
        let outcome = testing::run(&["metrics", &input, "--ri", "1.76", "--json"]);
        assert_eq!(outcome.exit, EXIT_OK, "{}", outcome.stderr);
        assert_eq!(outcome.files.len(), 0);
        let value: Value = serde_json::from_str(&outcome.stdout).expect("valid JSON");
        assert!(value["figures"]["table_up_windowing_pct"].is_number());
    }

    #[test]
    fn end_to_end_metrics_writes_csv_to_a_file() {
        let input = testing::write_design("metrics-e2e-csv.indicatrix", &testing::template());
        let target = testing::temp_path("metrics-e2e.csv")
            .to_string_lossy()
            .into_owned();
        let outcome = testing::run(&["metrics", &input, "--ri", "1.76", "--csv", "--out", &target]);
        assert_eq!(outcome.exit, EXIT_OK, "{}", outcome.stderr);
        assert_eq!(outcome.stdout, "");
        let csv = testing::file_text(&outcome, &target).expect("the CSV is written");
        assert_eq!(csv.lines().count(), 2);
    }

    #[test]
    fn end_to_end_metrics_with_tilt_adds_the_tilt_averages() {
        let input = testing::write_design("metrics-e2e-tilt.indicatrix", &testing::template());
        let outcome = testing::run(&["metrics", &input, "--ri", "1.76", "--tilt", "--json"]);
        assert_eq!(outcome.exit, EXIT_OK, "{}", outcome.stderr);
        let value: Value = serde_json::from_str(&outcome.stdout).expect("valid JSON");
        let figures = &value["figures"];
        for key in [
            "tilt_brilliance_pct",
            "tilt_windowing_pct",
            "tilt_extinction_pct",
        ] {
            let reading = figures[key].as_f64().unwrap_or(-1.0);
            assert!((0.0..=100.0).contains(&reading), "{key} = {reading}");
        }
    }

    #[test]
    fn an_unknown_material_is_a_usage_error() {
        let input = testing::write_design("metrics-e2e-unknown.indicatrix", &testing::template());
        let outcome = testing::run(&["metrics", &input, "--material", "Unobtainium"]);
        assert_eq!(outcome.exit, EXIT_USAGE);
        assert!(
            outcome.stderr.contains("unknown material"),
            "{}",
            outcome.stderr
        );
    }

    #[test]
    fn a_missing_file_is_an_io_error_and_an_empty_design_a_design_error() {
        let outcome = testing::run(&["metrics", "no-such-file.indicatrix", "--ri", "1.76"]);
        assert_eq!(outcome.exit, crate::outcome::EXIT_IO, "{}", outcome.stderr);
        let design = Design::new(
            indicatrix_cut_core::PreformSpec::block(2.0, 1.0, 4.0),
            indicatrix_cut_core::ScheduleMeta::standard_round_brilliant(),
            Vec::new(),
        );
        let input = testing::write_design("metrics-e2e-empty.indicatrix", &design);
        let outcome = testing::run(&["metrics", &input, "--ri", "1.76"]);
        assert_eq!(outcome.exit, EXIT_DESIGN, "{}", outcome.stderr);
        assert_eq!(outcome.stdout, "");
    }
}
