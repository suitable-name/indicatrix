//! The solve of a design and what it makes of the stone: shared by every command that needs a
//! solved, closed stone.
//!
//! [`analyze`] never fails. A design that does not solve is a result (`validate` reports it as
//! a Problem), so the failure is data on the [`Analysis`], and a command that needs a usable
//! stone asks [`Analysis::require_stone`].

use crate::outcome::CliError;
use indicatrix::geometry::{
    meet_solver::{SolveStrategy, SolvedTier},
    stone_metrics::{SolidMesh, SolidStatus, build_solid_mesh},
};
use indicatrix_cut_core::{
    DEFAULT_MIN_FACET_AREA_FRACTION_OF_W2, Design, ManufacturabilityWarning,
    manufacturability::check_manufacturability_available,
};

/// Whether the facets enclose a stone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Closure {
    /// A finite, watertight solid.
    Closed,
    /// Some planes run off without closing the stone.
    Unbounded,
    /// Bounded, but no usable volume.
    Degenerate,
    /// There is no stone to look at: the design did not solve, or the solver could not place a
    /// tier.
    NotSolved,
}

impl Closure {
    /// The word a report uses.
    #[must_use]
    pub const fn word(self) -> &'static str {
        match self {
            Self::Closed => "closed",
            Self::Unbounded => "not closed",
            Self::Degenerate => "degenerate",
            Self::NotSolved => "not solved",
        }
    }
}

/// A design, solved and checked.
#[derive(Debug)]
pub struct Analysis {
    /// One solved tier per design tier, or `None` when the design did not solve.
    pub solved: Option<Vec<SolvedTier>>,
    /// Why there is no usable stone, when there is none: the solver's sentence, or which tiers
    /// it could not place.
    pub failure: Option<String>,
    /// Whether the facets enclose a stone.
    pub closure: Closure,
    /// The solid, when it closes.
    pub mesh: Option<SolidMesh>,
    /// The manufacturability warnings. The checks that need no solve run even when there is
    /// none.
    pub warnings: Vec<ManufacturabilityWarning>,
}

/// Solves `design`, builds its solid and runs the manufacturability checks.
#[must_use]
pub fn analyze(design: &Design) -> Analysis {
    let solved = match design.solve() {
        Ok(solved) => solved,
        Err(error) => {
            return Analysis {
                solved: None,
                failure: Some(error.to_string()),
                closure: Closure::NotSolved,
                mesh: None,
                warnings: check_manufacturability_available(
                    design,
                    None,
                    DEFAULT_MIN_FACET_AREA_FRACTION_OF_W2,
                ),
            };
        }
    };
    let warnings = check_manufacturability_available(
        design,
        Some(&solved),
        DEFAULT_MIN_FACET_AREA_FRACTION_OF_W2,
    );
    let unplaced: Vec<String> = solved
        .iter()
        .enumerate()
        .filter(|(_, tier)| matches!(tier.strategy, SolveStrategy::Failed))
        .map(|(index, _)| indicatrix_editor::retarget::plan::tier_display_name(design, index))
        .collect();
    if !unplaced.is_empty() {
        return Analysis {
            failure: Some(format!(
                "the solver could not place {}: {}",
                crate::format::count_noun(unplaced.len(), "tier", "tiers"),
                unplaced.join(", ")
            )),
            solved: Some(solved),
            closure: Closure::NotSolved,
            mesh: None,
            warnings,
        };
    }
    let planes = design.planes_from_solved(&solved);
    let (closure, mesh, failure) = match build_solid_mesh(&planes) {
        SolidStatus::Closed(mesh) => (Closure::Closed, Some(mesh), None),
        SolidStatus::Unbounded { escaping } => (
            Closure::Unbounded,
            None,
            Some(format!(
                "{} of the facets run off without closing the stone",
                escaping.len()
            )),
        ),
        SolidStatus::Degenerate { .. } => (
            Closure::Degenerate,
            None,
            Some("the facets enclose no usable volume".to_string()),
        ),
    };
    Analysis {
        solved: Some(solved),
        failure,
        closure,
        mesh,
        warnings,
    }
}

impl Analysis {
    /// Whether the facets enclose a stone.
    #[must_use]
    pub const fn is_closed(&self) -> bool {
        matches!(self.closure, Closure::Closed)
    }

    /// The sentence saying why this is not a usable stone; `None` when it is.
    #[must_use]
    pub fn problem(&self, design: &Design) -> Option<String> {
        if design.tiers.is_empty() {
            return Some("the design has no facets".to_string());
        }
        if self.is_closed() {
            return None;
        }
        Some(
            self.failure
                .clone()
                .unwrap_or_else(|| "the design does not solve".to_string()),
        )
    }

    /// The solved tiers of a usable stone.
    ///
    /// # Errors
    ///
    /// [`CliError::design`] with [`Self::problem`]'s sentence when the design does not solve,
    /// does not close, or has no facets.
    pub fn require_stone(&self, design: &Design) -> Result<&[SolvedTier], CliError> {
        if let Some(problem) = self.problem(design) {
            return Err(CliError::design(format!(
                "the design is not a usable stone: {problem}"
            )));
        }
        self.solved
            .as_deref()
            .ok_or_else(|| CliError::design("the design does not solve"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing;
    use indicatrix::geometry::meet_solver::MeetConstraint;
    use indicatrix_cut_core::{ConstraintTier, PreformSpec, ScheduleMeta};

    #[test]
    fn the_template_solves_and_closes() {
        let design = testing::template();
        let analysis = analyze(&design);
        assert!(analysis.is_closed(), "{:?}", analysis.failure);
        assert_eq!(analysis.closure.word(), "closed");
        assert!(analysis.mesh.is_some());
        assert!(analysis.failure.is_none());
        assert_eq!(
            analysis.solved.as_ref().map(Vec::len),
            Some(design.tiers.len())
        );
        assert!(analysis.problem(&design).is_none());
        let solved = analysis.require_stone(&design).expect("a usable stone");
        assert_eq!(solved.len(), design.tiers.len());
    }

    #[test]
    fn a_design_without_facets_is_not_a_usable_stone() {
        let mut design = testing::template();
        design.tiers.clear();
        let analysis = analyze(&design);
        let problem = analysis.problem(&design).expect("a problem");
        assert!(problem.contains("no facets"), "{problem}");
        assert!(analysis.require_stone(&design).is_err());
    }

    #[test]
    fn a_design_with_no_scale_anchor_does_not_solve() {
        let tier = ConstraintTier {
            angle_deg: -41.0,
            name: "P1".to_string(),
            indices: vec![12.0, 36.0, 60.0, 84.0],
            constraint: MeetConstraint::MeetExisting,
            imported_meet: None,
            original_notes: None,
            detached: Vec::new(),
        };
        let design = Design::new(
            PreformSpec::block(2.0, 1.0, 4.0),
            ScheduleMeta::standard_round_brilliant(),
            vec![tier],
        );
        let analysis = analyze(&design);
        assert!(analysis.solved.is_none());
        assert_eq!(analysis.closure, Closure::NotSolved);
        let failure = analysis.failure.as_deref().expect("a reason");
        assert_ne!(failure, "");
        let error = analysis.require_stone(&design).expect_err("no stone");
        assert_eq!(error.code, crate::outcome::EXIT_DESIGN);
    }

    #[test]
    fn the_closure_words_are_distinct() {
        let words = [
            Closure::Closed.word(),
            Closure::Unbounded.word(),
            Closure::Degenerate.word(),
            Closure::NotSolved.word(),
        ];
        for (i, a) in words.iter().enumerate() {
            for b in &words[i + 1..] {
                assert_ne!(a, b);
            }
        }
    }
}
