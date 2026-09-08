//! [`MissingAnchor`], the error [`super::Design::solve`]/[`super::Design::resolve_dirty`]
//! return when a block has no explicit scale-reference tier -- see the parent
//! module's doc comment ("Scale anchoring") for why this is a hard failure
//! rather than a silently-invented default.

use indicatrix::geometry::meet_solver::Block;

/// One or more blocks [`super::Design::solve`] could not produce masts for.
///
/// Each named [`Block`] has at least one tier but none of them carry an explicit
/// [`indicatrix::geometry::meet_solver::MeetConstraint::ScaleReference`], so the block's translation along its own
/// normal is genuinely undetermined (see the module docs). Never fabricated past
/// this point -- the caller (the editor UI) must ask the user for a real
/// scale-reference tier on each named block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MissingAnchor {
    pub blocks: Vec<Block>,
}

impl std::fmt::Display for MissingAnchor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "no scale-reference tier for: ")?;
        for (i, block) in self.blocks.iter().enumerate() {
            if i > 0 {
                write!(f, ", ")?;
            }
            write!(
                f,
                "{}",
                match block {
                    Block::Crown => "crown",
                    Block::Pavilion => "pavilion",
                    Block::Girdle => "girdle",
                }
            )?;
        }
        Ok(())
    }
}

impl std::error::Error for MissingAnchor {}
