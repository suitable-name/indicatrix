//! `export`: the design as `.asc` cutting instructions, a `.indicatrix` file, a Gem Cut Studio
//! file or the cutting sheet as a web page.
//!
//! Every format is written from the same solve the other commands use, so a design that does
//! not solve and close is refused (exit 2) and nothing is written. The text of each format is
//! the one the editor writes:
//!
//! | Format | Written by |
//! |---|---|
//! | `asc` | `indicatrix_formats::asc::to_asc_string` on the editor's export schedule: the effective refractive index, the solved masts and concave tiers as footnotes, with the CRLF line ends `GemCAD` itself writes |
//! | `indicatrix` | the file the design was opened from, with the design swapped in (see [`Loaded::indicatrix_text`]) |
//! | `gcs` | `indicatrix_formats::gcs::to_gcs_string` on the same schedule `asc` is made from (experimental, as in the editor) |
//! | `html` | `indicatrix_editor::cut_sheet::cutting_sheet_document_with`: the cutting instructions with their four views, headed by the file's own title, designer, shape and notes (else the `.asc` header lines) and the `--date` text |
//!
//! The desktop editor also stamps the library entry a design came from into the footnotes of an
//! exported `.asc` or `.gcs`, and marks a schedule rebuilt from an angle table; a command line
//! has no such entry and no such table, so the CLI leaves both out.

use super::open;
use crate::{
    args::{ExportArgs, ExportFormat},
    load::Loaded,
    materials::Catalogue,
    outcome::{CliError, CommandResult, Outcome},
    stone::analyze,
};
use indicatrix::geometry::meet_solver::SolvedTier;
use indicatrix_cut_core::Design;
use indicatrix_editor::cut_sheet::cutting_sheet_document_with;
use indicatrix_formats::{
    asc::{AscSchedule, to_asc_string},
    gcs::to_gcs_string,
};

/// The schedule `.asc` and `.gcs` are written from: the effective refractive index (custom
/// materials taken into account), the solved masts, and the concave tiers as footnotes. The
/// desktop's Export builds the same schedule.
///
/// # Errors
///
/// [`CliError::design`] naming `kind` when the solved tiers do not match the design.
fn export_schedule(
    design: &Design,
    solved: &[SolvedTier],
    catalogue: &Catalogue,
    kind: &str,
) -> Result<AscSchedule, CliError> {
    let mut schedule = design
        .try_to_asc_schedule_from_solved_with(solved, catalogue.custom())
        .map_err(|error| CliError::design(format!("cannot write the design as {kind}: {error}")))?;
    design.append_concave_footnotes(&mut schedule);
    Ok(schedule)
}

/// The text of `loaded` in `format`. `date_text` is what the html sheet prints under its
/// title (empty prints no date); no other format reads it.
///
/// # Errors
///
/// [`CliError::design`] when the design is not a usable stone, or the format's writer refuses
/// it (a header or footnote it cannot hold, planes that do not enclose a stone).
fn export_text(
    loaded: &Loaded,
    catalogue: &Catalogue,
    format: ExportFormat,
    date_text: &str,
) -> Result<String, CliError> {
    let design = &loaded.design;
    let analysis = analyze(design);
    let solved = analysis.require_stone(design)?;
    match format {
        ExportFormat::Asc => {
            let schedule = export_schedule(design, solved, catalogue, ".asc")?;
            to_asc_string(&schedule).map_err(|error| {
                CliError::design(format!("cannot write the design as .asc: {error}"))
            })
        }
        ExportFormat::Indicatrix => loaded.indicatrix_text(design, catalogue),
        ExportFormat::Gcs => {
            let schedule = export_schedule(design, solved, catalogue, ".gcs")?;
            to_gcs_string(&schedule).map_err(|error| {
                CliError::design(format!("cannot write the design as .gcs: {error}"))
            })
        }
        ExportFormat::Html => Ok(cutting_sheet_document_with(
            design,
            solved,
            catalogue.custom(),
            &loaded.sheet_details(date_text),
        )),
    }
}

/// Runs `export` on an opened design.
///
/// # Errors
///
/// See [`export_text`].
fn execute(loaded: &Loaded, catalogue: &Catalogue, args: &ExportArgs) -> CommandResult {
    let text = export_text(
        loaded,
        catalogue,
        args.format,
        args.date.as_deref().unwrap_or_default(),
    )?;
    Ok(Outcome::default().with_file(args.out.clone(), text))
}

/// Runs `export`.
///
/// # Errors
///
/// [`CliError`] when the design cannot be opened or is not a usable stone, or the format's
/// writer refuses it.
pub fn run(args: &ExportArgs) -> CommandResult {
    let (loaded, catalogue) = open(&args.design, args.db.as_deref())?;
    execute(&loaded, &catalogue, args)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        load::from_bytes,
        outcome::{EXIT_DESIGN, EXIT_OK, EXIT_USAGE},
        testing,
    };
    use indicatrix_cut_core::{Design, PreformSpec, ScheduleMeta};

    fn loaded() -> Loaded {
        Loaded::in_memory(testing::template(), "round.indicatrix")
    }

    fn text_in(format: ExportFormat) -> String {
        export_text(&loaded(), &Catalogue::default(), format, "").expect("exports")
    }

    #[test]
    fn asc_text_opens_again_with_the_same_number_of_tiers() {
        let text = text_in(ExportFormat::Asc);
        let again = from_bytes("round.asc", text.as_bytes()).expect("the exported .asc opens");
        assert_eq!(again.design.tiers.len(), testing::template().tiers.len());
    }

    /// The desktop's Export builds the schedule with `to_asc_schedule_with`, adds the concave
    /// footnotes and writes it with `to_asc_string` (`native_io/export.rs`, `schedule_file_text`),
    /// which ends every line with CRLF as `GemCAD` does. The CLI's `.asc` must be those bytes.
    #[test]
    fn asc_text_is_what_the_editors_export_writes() {
        let design = testing::template();
        let mut schedule = design.to_asc_schedule_with(&[]).expect("the design solves");
        design.append_concave_footnotes(&mut schedule);
        let editor = to_asc_string(&schedule).expect("the editor's schedule is writable");
        let text = text_in(ExportFormat::Asc);
        assert_eq!(text, editor);
        assert!(text.contains("\r\n"), "CRLF line ends");
        assert!(
            !text.replace("\r\n", "").contains('\n'),
            "no bare line feed"
        );
    }

    #[test]
    fn indicatrix_text_opens_again_as_the_same_design() {
        let text = text_in(ExportFormat::Indicatrix);
        let again = from_bytes("round.indicatrix", text.as_bytes()).expect("opens");
        assert_eq!(
            testing::shape(&again.design),
            testing::shape(&testing::template())
        );
    }

    #[test]
    fn gcs_text_is_a_gem_cut_studio_file_that_opens_again() {
        let text = text_in(ExportFormat::Gcs);
        assert!(
            text.starts_with("<GemCutStudio"),
            "{}",
            &text[..text.len().min(60)]
        );
        let again = from_bytes("round.gcs", text.as_bytes()).expect("the exported .gcs opens");
        assert_ne!(again.design.tiers.len(), 0);
    }

    #[test]
    fn html_text_is_the_cutting_sheet() {
        let text = text_in(ExportFormat::Html);
        assert!(
            text.to_ascii_lowercase().contains("<html"),
            "{}",
            &text[..text.len().min(60)]
        );
    }

    /// The date is text the caller gives: none means no date line (so the output never depends
    /// on a clock), and the given text is printed under the title.
    #[test]
    fn the_html_sheet_prints_the_given_date_and_no_other() {
        let without = text_in(ExportFormat::Html);
        assert!(!without.contains("class=\"date\""), "{without}");
        let with = export_text(
            &loaded(),
            &Catalogue::default(),
            ExportFormat::Html,
            "October 2026",
        )
        .expect("exports");
        assert!(
            with.contains("<p class=\"date\">October 2026</p>"),
            "{with}"
        );
    }

    #[test]
    fn the_same_export_is_the_same_text_every_time() {
        for format in [ExportFormat::Asc, ExportFormat::Gcs, ExportFormat::Html] {
            assert_eq!(text_in(format), text_in(format), "{format:?}");
        }
    }

    #[test]
    fn a_design_that_is_not_a_stone_is_refused_in_every_format() {
        let empty = Design::new(
            PreformSpec::block(2.0, 1.0, 4.0),
            ScheduleMeta::standard_round_brilliant(),
            Vec::new(),
        );
        let loaded = Loaded::in_memory(empty, "empty.indicatrix");
        for format in [
            ExportFormat::Asc,
            ExportFormat::Indicatrix,
            ExportFormat::Gcs,
            ExportFormat::Html,
        ] {
            let error =
                export_text(&loaded, &Catalogue::default(), format, "").expect_err("no stone");
            assert_eq!(error.code, EXIT_DESIGN, "{format:?}: {}", error.message);
        }
    }

    #[test]
    fn end_to_end_export_writes_one_file_and_prints_nothing() {
        let input = testing::write_design("export-e2e.indicatrix", &testing::template());
        let target = testing::temp_path("export-e2e.asc")
            .to_string_lossy()
            .into_owned();
        let outcome = testing::run(&["export", &input, "--format", "asc", "--out", &target]);
        assert_eq!(outcome.exit, EXIT_OK, "{}", outcome.stderr);
        assert_eq!(outcome.stdout, "");
        assert_eq!(outcome.files.len(), 1);
        assert!(testing::file_text(&outcome, &target).is_some_and(|text| !text.is_empty()));
    }

    #[test]
    fn end_to_end_the_format_follows_the_file_extension_when_none_is_given() {
        let input = testing::write_design("export-e2e-ext.indicatrix", &testing::template());
        let target = testing::temp_path("export-e2e-ext.html")
            .to_string_lossy()
            .into_owned();
        let outcome = testing::run(&["export", &input, "--out", &target]);
        assert_eq!(outcome.exit, EXIT_OK, "{}", outcome.stderr);
        let text = testing::file_text(&outcome, &target).expect("written");
        assert!(text.to_ascii_lowercase().contains("<html"));
    }

    #[test]
    fn end_to_end_an_unknown_extension_without_a_format_is_a_usage_error() {
        let input = testing::write_design("export-e2e-bad.indicatrix", &testing::template());
        let target = testing::temp_path("export-e2e-bad.dat")
            .to_string_lossy()
            .into_owned();
        let outcome = testing::run(&["export", &input, "--out", &target]);
        assert_eq!(outcome.exit, EXIT_USAGE, "{}", outcome.stderr);
        assert_eq!(outcome.files.len(), 0);
    }
}
