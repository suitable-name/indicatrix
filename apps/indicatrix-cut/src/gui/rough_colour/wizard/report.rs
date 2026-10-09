//! "Export report": a Markdown file and PNG pictures in a folder the cutter picks. The content is
//! built here from the fit result; the pictures are rasters made by the window.

use super::{
    compare::{chip_rows, lovo_rows, model_rows, warning_rows},
    raster::{Rgba, to_png},
    zone_rows::zone_rows,
};
use indicatrix::optics::zoning::ZonedAbsorption;
use indicatrix_cut_core::rough_plan::colour_fit::solve::ColourFit;
use std::{
    fmt::Write as _,
    path::{Path, PathBuf},
};

/// What the report says.
pub struct ReportInput<'a> {
    /// The rough's name.
    pub rough_name: &'a str,
    /// The rig's name.
    pub rig_name: &'a str,
    /// The views' names, by rig view index.
    pub view_names: &'a [String],
    /// The fit.
    pub fit: &'a ColourFit,
    /// The zone geometry that was fitted.
    pub zoned: &'a ZonedAbsorption,
    /// The planned stone width in mm.
    pub planned_mm: f64,
    /// Notes about the masks, one line each.
    pub mask_notes: &'a [String],
    /// A date line, supplied by the caller (the report itself reads no clock).
    pub date: &'a str,
}

/// A picture of the report.
pub struct ReportImage {
    /// The file name in the folder.
    pub file_name: String,
    /// The picture.
    pub image: Rgba,
}

/// The name of the Markdown file.
pub const REPORT_FILE: &str = "rough-colour-report.md";

/// A file-name-safe piece of `text`: letters and digits kept, the rest becomes `-`.
#[must_use]
pub fn slug(text: &str) -> String {
    let mut out = String::new();
    for c in text.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    let trimmed = out.trim_matches('-');
    if trimmed.is_empty() {
        "view".to_owned()
    } else {
        trimmed.to_owned()
    }
}

/// The file name of a view's picture: `"photo-3-x-upper.png"`.
#[must_use]
pub fn image_file_name(kind: &str, view: usize, view_name: &str) -> String {
    format!("{kind}-{}-{}.png", view + 1, slug(view_name))
}

/// The report as Markdown.
#[must_use]
pub fn markdown(input: &ReportInput<'_>) -> String {
    let fit = input.fit;
    let mut out = String::new();
    // Writing to a `String` cannot fail, so the `fmt::Result`s are discarded.
    let _ = writeln!(out, "# Rough colour report: {}\n", input.rough_name);
    let _ = writeln!(out, "{}\n", input.date);
    out.push_str("## Setup\n\n");
    let _ = writeln!(out, "- Rig: {}", input.rig_name);
    let _ = writeln!(out, "- Views: {}", input.view_names.join(", "));
    let _ = writeln!(out, "- Zones: {}", fit.n_zones);
    let _ = writeln!(
        out,
        "- Residual: rms {:.2} sigma, structure score {:.2}",
        fit.residual_rms, fit.structured_score
    );
    if let Some(roughness) = &fit.roughness {
        let _ = writeln!(
            out,
            "- Surface roughness (GGX alpha): {:.3}",
            roughness.roughness
        );
    }
    out.push('\n');
    if !input.mask_notes.is_empty() {
        out.push_str("## Masks\n\n");
        for note in input.mask_notes {
            let _ = writeln!(out, "- {note}");
        }
        out.push('\n');
    }
    out.push_str("## Zones\n\n");
    for row in zone_rows(input.zoned) {
        let _ = writeln!(out, "- {}: {}", row.title, row.summary);
    }
    out.push('\n');
    out.push_str("## Models\n\n");
    for row in model_rows(&fit.comparison) {
        let _ = writeln!(out, "- {}", row.text);
    }
    out.push('\n');
    if let Some(lovo) = &fit.lovo {
        out.push_str("## Leave one view out\n\n");
        for row in lovo_rows(lovo, input.view_names) {
            let _ = writeln!(out, "- {}", row.text);
        }
        out.push('\n');
    }
    let _ = writeln!(
        out,
        "## Predicted colours (reference 7 mm and {:.0} mm)\n",
        input.planned_mm
    );
    out.push_str("| Colour | Lab and uncertainty | sRGB |\n|---|---|---|\n");
    for chip in chip_rows(fit) {
        let _ = writeln!(
            out,
            "| {} | {} | #{:02x}{:02x}{:02x} |",
            chip.label,
            chip.text.replace('\u{b1}', "+/-"),
            chip.srgb[0],
            chip.srgb[1],
            chip.srgb[2]
        );
    }
    out.push('\n');
    let warnings = warning_rows(fit);
    if !warnings.is_empty() {
        out.push_str("## Warnings\n\n");
        for row in warnings {
            let _ = writeln!(out, "- {}", row.text);
        }
        out.push('\n');
    }
    out.push_str("## Pictures\n\n");
    for (i, name) in input.view_names.iter().enumerate() {
        let _ = writeln!(
            out,
            "### {name}\n\n![photo]({}) ![render]({}) ![difference]({})\n",
            image_file_name("photo", i, name),
            image_file_name("render", i, name),
            image_file_name("difference", i, name)
        );
    }
    out
}

/// Writes the Markdown and the pictures into `folder` (created if missing). Returns the files
/// written.
///
/// # Errors
///
/// A sentence naming the file that failed.
pub fn write_report(
    folder: &Path,
    markdown: &str,
    images: &[ReportImage],
) -> Result<Vec<PathBuf>, String> {
    std::fs::create_dir_all(folder)
        .map_err(|e| format!("Could not create {}: {e}", folder.display()))?;
    let mut written = Vec::with_capacity(images.len() + 1);
    for picture in images {
        let bytes = to_png(&picture.image)?;
        let path = folder.join(&picture.file_name);
        std::fs::write(&path, bytes)
            .map_err(|e| format!("Could not write {}: {e}", path.display()))?;
        written.push(path);
    }
    let path = folder.join(REPORT_FILE);
    std::fs::write(&path, markdown)
        .map_err(|e| format!("Could not write {}: {e}", path.display()))?;
    written.push(path);
    Ok(written)
}

#[cfg(test)]
mod tests {
    use super::{super::compare::fixtures::sample_fit, *};
    use indicatrix::optics::{absorption::AbsorptionTensor, zoning::ZoneAbsorption};

    fn input<'a>(
        fit: &'a ColourFit,
        zoned: &'a ZonedAbsorption,
        names: &'a [String],
        notes: &'a [String],
    ) -> ReportInput<'a> {
        ReportInput {
            rough_name: "Tourmaline 1",
            rig_name: "Eight views",
            view_names: names,
            fit,
            zoned,
            planned_mm: 12.0,
            mask_notes: notes,
            date: "2026-10-09",
        }
    }

    #[test]
    fn the_markdown_has_every_section_and_the_numbers() {
        let fit = sample_fit();
        let zoned = ZonedAbsorption::new(ZoneAbsorption::per_mm(AbsorptionTensor::isotropic(
            Vec::new(),
        )));
        let names = vec!["+X upper".to_owned(), "-X lower".to_owned()];
        let notes = vec!["+X upper: 12 % of the pixels are masked".to_owned()];
        let text = markdown(&input(&fit, &zoned, &names, &notes));
        for needle in [
            "# Rough colour report: Tourmaline 1",
            "## Setup",
            "- Rig: Eight views",
            "## Masks",
            "## Zones",
            "- Base zone:",
            "## Models",
            "## Leave one view out",
            "+X upper: predicted from the other views",
            "## Predicted colours (reference 7 mm and 12 mm)",
            "| Base zone, 7 mm, A |",
            "+/- 1.8 dE",
            "## Warnings",
            "The fit did not converge.",
            "![photo](photo-1-x-upper.png)",
            "![difference](difference-2-x-lower.png)",
        ] {
            assert!(text.contains(needle), "missing {needle:?} in\n{text}");
        }
        assert!(!text.contains("GemRay"));
    }

    #[test]
    fn slugs_and_picture_names() {
        assert_eq!(slug("+X upper"), "x-upper");
        assert_eq!(slug("***"), "view");
        assert_eq!(slug("A  b/c"), "a-b-c");
        assert_eq!(
            image_file_name("render", 2, "-Y lower"),
            "render-3-y-lower.png"
        );
    }

    #[test]
    fn the_report_is_written_to_a_folder() {
        let dir =
            std::env::temp_dir().join(format!("indicatrix-report-test-{}", std::process::id()));
        let images = vec![ReportImage {
            file_name: "photo-1-a.png".to_owned(),
            image: Rgba::filled(2, 2, [1, 2, 3, 255]),
        }];
        let written = write_report(&dir, "# hello\n", &images).unwrap();
        assert_eq!(written.len(), 2);
        assert_eq!(
            std::fs::read_to_string(dir.join(REPORT_FILE)).unwrap(),
            "# hello\n"
        );
        let png = std::fs::read(dir.join("photo-1-a.png")).unwrap();
        assert_eq!(&png[..4], b"\x89PNG");
        let _ = std::fs::remove_dir_all(&dir);
        let bad = vec![ReportImage {
            file_name: "x.png".to_owned(),
            image: Rgba::filled(0, 0, [0; 4]),
        }];
        assert!(write_report(&dir, "x", &bad).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
