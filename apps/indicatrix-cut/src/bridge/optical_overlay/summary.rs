//! The one-line summary shown under the overlay panel's legend.
use indicatrix::color::metrics::OpticalImage;
use std::fmt::Write as _;

/// The visible-area shares of `map`, plus its scintillation when the layer is on and the map
/// computed it. The scintillation figure is the image-domain aggregate, so the text says it
/// is not the HUD's grid value.
pub(super) fn summary_text(map: &OpticalImage, with_scintillation: bool) -> String {
    let f = map.total.fractions();
    let mut text = format!(
        "Visible-Area Shares: W {:.1}% · E {:.1}% · B {:.1}% · Unknown {:.1}%",
        f[0] * 100.0,
        f[1] * 100.0,
        f[2] * 100.0,
        f[3] * 100.0
    );
    if with_scintillation && map.scint_computed {
        let _ = write!(
            text,
            " · Visible-Area Scintillation (Map) {:.0}% (not the HUD value: other sampling domain and observer)",
            map.scintillation_pct()
        );
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use indicatrix::color::metrics::OpticalPixel;

    fn map(scint_computed: bool) -> OpticalImage {
        let pixel = OpticalPixel {
            hits: 4,
            classes: [1, 1, 2, 0],
            scint_returned: if scint_computed { 2 } else { 0 },
            scint_poses: if scint_computed { 5 } else { 0 },
            ..OpticalPixel::default()
        };
        OpticalImage {
            width: 1,
            height: 1,
            samples: 4,
            fire_computed: false,
            scint_computed,
            origin: [0, 0],
            total: pixel,
            pixels: vec![pixel],
        }
    }

    #[test]
    fn the_shares_line_is_unchanged_without_scintillation() {
        let text = summary_text(&map(false), false);
        assert_eq!(
            text,
            "Visible-Area Shares: W 25.0% · E 25.0% · B 50.0% · Unknown 0.0%"
        );
        // Asked for but not computed: nothing is invented.
        assert_eq!(summary_text(&map(false), true), text);
        // Computed but the layer is off: not shown.
        assert_eq!(summary_text(&map(true), false), text);
    }

    #[test]
    fn aggregate_is_not_the_grid_value_and_says_so() {
        let text = summary_text(&map(true), true);
        assert!(text.contains("Visible-Area Scintillation (Map)"), "{text}");
        assert!(text.contains("not the HUD value"), "{text}");
        assert!(text.starts_with("Visible-Area Shares:"), "{text}");
    }
}
