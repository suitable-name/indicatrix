//! [`MissingAnchor`], the error [`super::Design::solve`]/[`super::Design::resolve_dirty`]
//! return when a block has no explicit scale-reference tier -- see the parent
//! module's doc comment ("Scale anchoring") for why this is a hard failure
//! rather than a silently-invented default.

use indicatrix::geometry::meet_solver::{Block, classify_blocks};

/// One or more blocks [`super::Design::solve`] could not produce masts for.
///
/// Each named [`Block`] has at least one tier but none of them carry an explicit
/// [`indicatrix::geometry::meet_solver::MeetConstraint::ScaleReference`], so the block's translation along its own
/// normal is genuinely undetermined (see the module docs). Never fabricated past
/// this point -- the caller (the editor UI) must ask the user for a real
/// scale-reference tier on each named block.
///
/// [`std::fmt::Display`] gives the actionable remedy sentence for every named
/// block (e.g. `"Pavilion has no anchor: add a tier with an exact scale
/// value."`, also available one block at a time via [`Self::block_sentence`]);
/// when the caller also needs to know exactly which of
/// [`super::Design::tiers`] belong to each named block -- to mark only those,
/// and nothing else -- see [`Self::block_details`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MissingAnchor {
    /// Every crown/pavilion/girdle block with no scale-reference tier.
    pub blocks: Vec<Block>,
}

impl std::fmt::Display for MissingAnchor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for (i, &block) in self.blocks.iter().enumerate() {
            if i > 0 {
                write!(f, " ")?;
            }
            write!(f, "{}", BlockSentence(block))?;
        }
        Ok(())
    }
}

impl std::error::Error for MissingAnchor {}

impl MissingAnchor {
    /// [`Self::blocks`], each paired with the indices into `design.tiers`
    /// that [`classify_blocks`] puts in that block -- so a caller (the
    /// editor) can mark exactly the tiers responsible for each named block
    /// without re-deriving the crown/pavilion/girdle classification itself.
    ///
    /// `design` must be the design [`super::Design::solve`] or
    /// [`super::Design::resolve_dirty`] actually returned this error for --
    /// this recomputes the same classification (over
    /// [`super::Design::meet_tier_inputs`]) that raised the error in the
    /// first place, so it always agrees with `self.blocks`.
    #[must_use]
    pub fn block_details(&self, design: &super::Design) -> Vec<(Block, Vec<usize>)> {
        let inputs = design.meet_tier_inputs();
        let classified = classify_blocks(&inputs);
        self.blocks
            .iter()
            .map(|&block| (block, tier_indices_for_block(&classified, block)))
            .collect()
    }

    /// The one-sentence remedy for a single block, independent of whether
    /// this particular [`MissingAnchor`] actually names it -- e.g. a per-row
    /// tooltip keyed off one of [`Self::block_details`]'s [`Block`]s, without
    /// wrapping it back into a whole [`MissingAnchor`] just to read its
    /// [`std::fmt::Display`] text.
    #[must_use]
    pub fn block_sentence(block: Block) -> String {
        BlockSentence(block).to_string()
    }
}

/// Indices of every tier `classified` (parallel to `design.tiers`, from
/// [`classify_blocks`]) puts in `block`.
fn tier_indices_for_block(classified: &[Block], block: Block) -> Vec<usize> {
    classified
        .iter()
        .enumerate()
        .filter_map(|(i, &b)| (b == block).then_some(i))
        .collect()
}

/// The one-sentence remedy for a single unanchored block, shared by
/// [`MissingAnchor`]'s [`std::fmt::Display`] impl and [`MissingAnchor::block_sentence`]
/// so the two can never say something different about the same block.
struct BlockSentence(Block);

impl std::fmt::Display for BlockSentence {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let name = match self.0 {
            Block::Crown => "Crown",
            Block::Pavilion => "Pavilion",
            Block::Girdle => "Girdle",
        };
        write!(
            f,
            "{name} has no anchor: add a tier with an exact scale value."
        )
    }
}
