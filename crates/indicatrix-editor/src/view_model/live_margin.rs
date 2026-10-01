//! The live critical-angle guidance under the Tier form's Angle field.
//!
//! Extracted from the desktop's `tier_actions::misc::setup_angle_live_preview_callback`
//! so the web inspector shows the same bar for a value typed but not yet saved as a saved
//! row's MARGIN cell does.

use super::row_format::representative_crown_and_pavilion_angles_deg;
use indicatrix_cut_core::{
    Design, Risk, crown_window_margin_deg, crown_windowing_risk, tier_margin_deg, windowing_risk,
};

/// What the live margin bar shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveMargin {
    /// The signed margin, `"+3.2\u{b0}"`.
    pub text: String,
    /// `0` safe, `1` marginal, `2` windows.
    pub level: i32,
    /// A crown margin is an estimate against the design's representative pavilion angle.
    pub is_estimate: bool,
}

const fn level_of(risk: Risk) -> i32 {
    match risk {
        Risk::Safe => 0,
        Risk::Marginal => 1,
        Risk::Windows => 2,
    }
}

/// The bar for the angle typed as `angle_text` on `design` at refractive index `n_d`.
///
/// A pavilion angle (`< 0`) reads the plain table-only critical-angle margin; a crown
/// angle (`> 0`) reads the crown-window ESTIMATE against the design's own representative
/// pavilion angle -- the same functions a saved row's MARGIN cell uses. `None` (nothing to
/// show) for text that is not a number, exactly zero, or a crown angle in a design with no
/// pavilion tier to estimate against.
#[must_use]
pub fn angle_live_margin(design: &Design, n_d: f64, angle_text: &str) -> Option<LiveMargin> {
    let angle_deg: f64 = angle_text.trim().parse().ok()?;
    if angle_deg < 0.0 {
        Some(LiveMargin {
            text: format!("{:+.1}\u{b0}", tier_margin_deg(angle_deg, n_d)),
            level: level_of(windowing_risk(angle_deg, n_d)),
            is_estimate: false,
        })
    } else if angle_deg > 0.0 {
        let (_, pavilion_deg) = representative_crown_and_pavilion_angles_deg(design);
        let pavilion_deg = pavilion_deg?;
        Some(LiveMargin {
            text: format!(
                "{:+.1}\u{b0}",
                crown_window_margin_deg(pavilion_deg, angle_deg, n_d)
            ),
            level: level_of(crown_windowing_risk(pavilion_deg, angle_deg, n_d)),
            is_estimate: true,
        })
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::EditorSession;

    fn design() -> Design {
        EditorSession::from_template(
            indicatrix_cut_core::FreshDesignSpec {
                gear_teeth: 96,
                symmetry_order: 8,
                mirror: true,
                material: indicatrix_cut_core::MaterialSelection::none(),
                preform: indicatrix_cut_core::PreformSpec::cylinder(96, 1.5, 1.0, 1.5),
            },
            1,
        )
        .design
    }

    #[test]
    fn a_pavilion_angle_reads_the_plain_margin_and_a_crown_angle_an_estimate() {
        let design = design();
        let n_d = design.effective_refractive_index();
        let pavilion = angle_live_margin(&design, n_d, "-43.0").expect("a pavilion bar");
        assert!(!pavilion.is_estimate);
        assert_eq!(
            pavilion.text,
            format!("{:+.1}\u{b0}", tier_margin_deg(-43.0, n_d))
        );
        let crown = angle_live_margin(&design, n_d, " 34.5 ").expect("a crown bar");
        assert!(crown.is_estimate);
        assert!((0..=2).contains(&crown.level));
    }

    #[test]
    fn nothing_is_shown_for_text_zero_or_a_crown_without_a_pavilion() {
        let design = design();
        let n_d = design.effective_refractive_index();
        assert_eq!(angle_live_margin(&design, n_d, "abc"), None);
        assert_eq!(angle_live_margin(&design, n_d, "0"), None);
        assert_eq!(angle_live_margin(&design, n_d, ""), None);
        let empty = EditorSession::fresh().design;
        assert_eq!(angle_live_margin(&empty, 1.54, "34.5"), None);
    }
}
