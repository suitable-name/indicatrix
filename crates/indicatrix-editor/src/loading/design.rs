//! Builds a [`Design`] from a bare `.asc` file's own text, with the default preform
//! that bounds it.
//!
//! The desktop's catalogue-record path (a vault record, possibly
//! with a native sidecar attached) stays with the desktop and calls into this.

use indicatrix_cut_core::{Design, PreformSpec};

/// A preform sized to bound a design that may already fully specify its own closed shape
/// (a real `.asc` file's tiers, or a placeholder reconstruction).
///
/// Generously large relative to the "mast order 1" scale every facet offset uses (see
/// `indicatrix_cut_core::preform`'s module doc comment), so this backstop rough does not
/// itself clip a single facet of an already-complete design -- only a schedule that's
/// missing a closing facet in some direction would ever touch its walls.
///
/// `length_over_width` is read from `lw_ratio` (a catalogue's recorded L/W) when it
/// parses as a positive finite number, so an oval design's preform isn't needlessly
/// round; `1.0` (round/square) otherwise.
#[must_use]
pub fn default_preform_for_schedule(
    schedule: &indicatrix_formats::asc::AscSchedule,
    lw_ratio: Option<&str>,
) -> PreformSpec {
    let length_over_width = lw_ratio
        .and_then(|s| s.trim().parse::<f64>().ok())
        .filter(|v| v.is_finite() && *v > 0.0)
        .unwrap_or(1.0);
    PreformSpec::cylinder_for_schedule(schedule, 3.0, length_over_width, 3.0)
}

/// A resolved design plus where its schedule came from.
pub struct LoadedDesign {
    /// The design.
    pub design: Design,
    /// `true` iff no real `.asc` was found and `design`'s schedule came from an
    /// angle-table placeholder reconstruction (every mast `0.0`).
    pub used_placeholder: bool,
    /// The real `.asc`'s own bare file name, `None` on the placeholder path.
    pub asc_filename: Option<String>,
    /// The real `.asc`'s exact original text, `None` on the placeholder path; fed to
    /// `indicatrix_cut_core::save_paired` so a native save can leave the `.asc`
    /// half byte-for-byte untouched.
    pub original_asc_text: Option<String>,
}

/// Builds a [`LoadedDesign`] from a real `.asc` file's own text, always along the
/// "real schedule" outcome (`used_placeholder: false`).
///
/// # Errors
///
/// The parse failure's own message when `text` does not parse as `.asc` cutting
/// instructions at all.
pub fn design_from_asc_text(
    file_name: &str,
    text: &str,
    lw_ratio: Option<&str>,
) -> Result<LoadedDesign, String> {
    let schedule = indicatrix_formats::asc::parse_asc(text).map_err(|e| e.to_string())?;
    let preform = default_preform_for_schedule(&schedule, lw_ratio);
    Ok(LoadedDesign {
        design: Design::from_asc_schedule(preform, &schedule),
        used_placeholder: false,
        asc_filename: Some(file_name.to_string()),
        original_asc_text: Some(text.to_string()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn design_from_asc_text_parses_a_real_schedule() {
        let text = "GemCad 5.0\ng 96 0.0\ny 4 y\nI 1.54\na -41.000000 0.64991234 92 n 1 84\n";
        let loaded = design_from_asc_text("design.asc", text, None).unwrap();
        assert!(!loaded.used_placeholder);
        assert_eq!(loaded.asc_filename.as_deref(), Some("design.asc"));
        assert_eq!(loaded.original_asc_text.as_deref(), Some(text));
        assert_eq!(loaded.design.tiers.len(), 1);
    }

    #[test]
    fn design_from_asc_text_rejects_unparseable_text() {
        assert!(design_from_asc_text("bad.asc", "not a real .asc schedule", None).is_err());
    }
}
