//! The live critical-angle guidance under the Tier form's Angle field.
//!
//! Extracted from the desktop's `tier_actions::misc::setup_angle_live_preview_callback`
//! so the web inspector shows the same bar for a value typed but not yet saved as a saved
//! row's MARGIN cell does.

use super::row_format::representative_crown_and_pavilion_angles_deg;
use crate::loading::eval_number;
use indicatrix_cut_core::{
    Design, Risk, crown_window_margin_deg, crown_windowing_risk,
    design::{ExprError, TierId},
    tier_margin_deg, windowing_risk,
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

/// The signed angle the Angle field's text stands for, for the live bar.
///
/// A number or a calculation over numbers (`41.5 + 0.3`) is itself. Text starting with `=`
/// is a RELATION: the angle it would give a tier, on the side (pavilion or crown) of the
/// first tier it reads -- the bar cannot know which tier is being edited, and a relation
/// nearly always follows a tier of its own block. `None` for anything that is not an angle.
fn typed_angle_deg(design: &Design, text: &str) -> Option<f64> {
    if text.trim_start().starts_with('=') {
        return relation_angle_deg(design, text);
    }
    eval_number(text, None).ok()
}

/// The angle the relation `text` (with or without its leading `=`) would give a tier.
///
/// The angle is signed like the first tier the relation reads. `None` when the relation
/// cannot be read or gives no facet angle (more than 0 degrees and at most 90). The Tier
/// form uses it as the starting angle of a NEW tier typed as a relation.
#[must_use]
pub fn relation_angle_deg(design: &Design, text: &str) -> Option<f64> {
    let relation = design.parse_relation(text).ok()?;
    let mut lookup = |id: &TierId| -> Result<f64, ExprError> {
        let tier = design
            .index_of_tier_id(*id)
            .and_then(|position| design.tiers.get(position))
            .ok_or_else(|| ExprError::UnknownName("a tier is missing".to_owned()))?;
        Ok(tier.angle_deg.abs())
    };
    let magnitude = relation.angle.eval(&mut lookup).ok()?;
    if magnitude <= 0.0 || magnitude > 90.0 {
        return None;
    }
    let first_read = relation
        .references()
        .iter()
        .filter_map(|id| design.index_of_tier_id(*id))
        .min()
        .and_then(|position| design.tiers.get(position))?;
    Some(if first_read.angle_deg.is_sign_negative() {
        -magnitude
    } else {
        magnitude
    })
}

/// The bar for the angle typed as `angle_text` on `design` at refractive index `n_d`.
///
/// The text may be a number, arithmetic (`41.5 + 0.3`) or a relation (`=P1 - 2`, see
/// [`typed_angle_deg`]). A pavilion angle (`< 0`) reads the plain table-only critical-angle
/// margin; a crown angle (`> 0`) reads the crown-window ESTIMATE against the design's own
/// representative pavilion angle -- the same functions a saved row's MARGIN cell uses.
/// `None` (nothing to show) for text that is not an angle, exactly zero, or a crown angle
/// in a design with no pavilion tier to estimate against.
#[must_use]
pub fn angle_live_margin(design: &Design, n_d: f64, angle_text: &str) -> Option<LiveMargin> {
    let angle_deg = typed_angle_deg(design, angle_text)?;
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

    /// A pavilion tier P1 at 43 degrees and a crown tier C1 at 34.5.
    fn two_tiers() -> Design {
        use indicatrix::geometry::meet_solver::MeetConstraint;
        use indicatrix_cut_core::{ConstraintTier, Edit};
        let mut session = EditorSession::fresh();
        for (index, (name, angle)) in [("P1", -43.0), ("C1", 34.5)].into_iter().enumerate() {
            let tier = ConstraintTier {
                angle_deg: angle,
                name: name.to_owned(),
                indices: vec![0.0, 24.0],
                constraint: MeetConstraint::ScaleReference(0.65),
                imported_meet: None,
                original_notes: None,
                detached: Vec::new(),
            };
            session
                .apply(Edit::AddTier { index, tier })
                .expect("a tier is added");
        }
        session.design
    }

    #[test]
    fn arithmetic_reads_the_bar_of_its_result() {
        let design = two_tiers();
        let n_d = design.effective_refractive_index();
        let calculated = angle_live_margin(&design, n_d, "-40 + 0.5").expect("a bar");
        assert_eq!(
            Some(calculated),
            angle_live_margin(&design, n_d, "-39.5"),
            "the sum reads like the number it makes"
        );
        assert_eq!(angle_live_margin(&design, n_d, "-40 +"), None);
    }

    #[test]
    fn a_relation_reads_the_bar_of_the_angle_it_would_give() {
        let design = two_tiers();
        let n_d = design.effective_refractive_index();
        // P1 is a pavilion tier, so a relation on it stays on the pavilion side.
        assert_eq!(
            angle_live_margin(&design, n_d, "=P1 - 3"),
            angle_live_margin(&design, n_d, "-40")
        );
        let crown = angle_live_margin(&design, n_d, "= C1 + 1.5").expect("a crown bar");
        assert!(crown.is_estimate);
        assert_eq!(Some(crown), angle_live_margin(&design, n_d, "36"));
        // Not a usable relation: nothing to show.
        for text in ["=", "=Q9 - 3", "=P1 - 60", "=P1 +", "=P1 - 43"] {
            assert_eq!(angle_live_margin(&design, n_d, text), None, "{text:?}");
        }
    }
}
