//! Cutting mode's pure model: a solved design's cutting instructions as steps, one per page.
//!
//! [`build_plan`] turns a design and its masts into a [`CuttingPlan`]: one [`CuttingStep`] per
//! tier, in the order the cutting sheet lists them -- [`Design::cutting_order`] for every
//! design, planar or concave, the very order [`Design::preview_steps`] gives the Cut slider --
//! with the figures a cutter reads at the machine: the tier's code (`P1`, `G1`, `C1`, `T`),
//! side, the angle as the sheet prints it (unsigned), the indices, the instruction (the tier's
//! name, then what it meets), depth and mast, the cheater offset, the cutter's note and, for a
//! concave tier, the tool line.
//!
//! Marks do not live in the design file. Each step carries a [`CuttingStep::key`] (the stable
//! tier id) and a [`CuttingStep::signature`] (a fingerprint of the values a cutter works to), and
//! [`progress::Progress`] compares the signature stored with a mark to the one now: a tier whose
//! angle, indices, depth or tool changed since it was marked reads "changed", not "done". A
//! note, a rename or a different meet wording changes no figure at the machine, so none of them
//! touches the signature.
//!
//! - [`progress`]: marks, ticks, counting and the resume position.
//! - [`display`]: the texts one page shows.
//! - [`dial`]: the index wheel with a step's indices marked.
//!
//! No GUI types, clock or filesystem: the desktop maps these to its own rows and stores the
//! marks in the library.

pub mod dial;
pub mod display;
pub mod progress;

#[cfg(test)]
mod tests;

use crate::view_model::row_format::format_index_value;
use indicatrix::{
    geometry::meet_solver::{Block, SolvedTier, classify_blocks},
    optics::materials::GemMaterial,
};
use indicatrix_cut_core::{
    ConcaveRowInfo, CutSheetRow, Design, TierId,
    design::{ConcaveTier, TierRef},
};
use std::fmt::Write as _;

/// FNV-1a 64-bit offset basis: a hash that is the same on every machine and build, unlike
/// `std`'s randomised hasher, because the fingerprint is stored in the library.
const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
/// FNV-1a 64-bit prime.
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

/// Which part of the stone a step cuts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepSide {
    /// Above the girdle.
    Crown,
    /// Below the girdle.
    Pavilion,
    /// The girdle itself.
    Girdle,
}

impl StepSide {
    /// The word the page and the cutting sheet use.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Crown => "Crown",
            Self::Pavilion => "Pavilion",
            Self::Girdle => "Girdle",
        }
    }

    const fn from_block(block: Block) -> Self {
        match block {
            Block::Crown => Self::Crown,
            Block::Pavilion => Self::Pavilion,
            Block::Girdle => Self::Girdle,
        }
    }
}

/// One index-wheel position a step cuts at.
#[derive(Debug, Clone, PartialEq)]
pub struct StepIndex {
    /// The position on the wheel (a whole tooth, or a fraction of one).
    pub value: f64,
    /// The position as the cutting sheet prints it: `12`, or `11.50`.
    pub text: String,
    /// The key its tick is stored under: `<step key>#<position>`.
    pub key: String,
}

/// One page of cutting mode: a tier to cut.
#[derive(Debug, Clone, PartialEq)]
pub struct CuttingStep {
    /// Position in the cutting sequence, from 1 (the cutting sheet's `#` column).
    pub number: usize,
    /// The stable key marks are stored under: `t<id>` for a flat tier, `c<id>` for a concave
    /// one, from the tier's never-reused id.
    pub key: String,
    /// A fingerprint of the cutting values (16 hexadecimal digits); see the module docs.
    pub signature: String,
    /// Which tier the step cuts.
    pub tier: TierRef,
    /// The tier's row in the tier table: the flat tiers first, the concave tiers after them.
    pub table_row: usize,
    /// The tier's code as the cutting sheet's label column shows it (`P1`, `G1`, `C1`, `T`,
    /// `Culet`, a concave tier's `P4`); the page heading and the stone caption use it.
    pub code: String,
    /// The tier's own name (`(unnamed)` for a tier without one). The page shows it in front of
    /// the instruction text ([`Self::meet`]), not as the heading.
    pub name: String,
    /// Crown, pavilion or girdle.
    pub side: StepSide,
    /// The angle from the girdle plane in degrees, unsigned, as the sheet prints it.
    pub angle_deg: f64,
    /// The index positions, at least one (a tier listing none is one facet at position 0).
    pub indices: Vec<StepIndex>,
    /// The sheet's instruction text: the tier's own name, then what the facet closes against
    /// (`Crown Main: Meet P1, P2`), or the concave tier's own instructions.
    pub meet: String,
    /// The solved mast in the design's units; `None` for a concave tier, which has none.
    pub mast: Option<f64>,
    /// The cutting depth in millimetres; `None` without a girdle diameter or for a concave tier.
    pub depth_mm: Option<f64>,
    /// The cheater (azimuth) offset in degrees, when the tier has one.
    pub cheater_offset_deg: Option<f64>,
    /// The cutter's own note on the tier; empty when there is none.
    pub notes: String,
    /// A concave tier's tool line (code, azimuth, displacement, size and motion).
    pub tool_line: Option<String>,
}

/// All the steps of a design and what the index wheel needs to draw them.
#[derive(Debug, Clone, PartialEq)]
pub struct CuttingPlan {
    /// The steps in cutting order.
    pub steps: Vec<CuttingStep>,
    /// The index gear's tooth count.
    pub gear_teeth: u32,
}

/// Builds the cutting plan of `design` from its solved masts `solved` (one per flat tier).
/// `custom` is the caller's own catalogue materials, as for the cutting sheet.
///
/// `None` when `solved` does not fit the design's tiers (a stale solve), or the design has no
/// tier to cut.
#[must_use]
pub fn build_plan(
    design: &Design,
    solved: &[SolvedTier],
    custom: &[GemMaterial],
) -> Option<CuttingPlan> {
    let sheet = design.try_cutting_sheet_with(solved, custom).ok()?;
    // `preview_steps()` is `cutting_order()`, the order the sheet's rows come in.
    let order = design.preview_steps();
    if order.is_empty() || order.len() != sheet.rows.len() {
        return None;
    }
    let blocks = classify_blocks(&design.meet_tier_inputs());
    let steps = order
        .into_iter()
        .zip(sheet.rows)
        .enumerate()
        .map(|(position, (tier, row))| step_from_row(design, &blocks, position, tier, row))
        .collect();
    Some(CuttingPlan {
        steps,
        gear_teeth: design.meta.gear_teeth_abs(),
    })
}

/// One step from the sheet row of the tier at `position` in the cutting order.
fn step_from_row(
    design: &Design,
    blocks: &[Block],
    position: usize,
    tier: TierRef,
    row: CutSheetRow,
) -> CuttingStep {
    let (key, side, table_row, notes) = match tier {
        TierRef::Flat(i) => (
            id_key('t', design.tier_id_at(i), i),
            blocks
                .get(i)
                .copied()
                .map_or(StepSide::Girdle, StepSide::from_block),
            i,
            design.tier_note(i).unwrap_or_default().trim().to_owned(),
        ),
        TierRef::Concave(i) => (
            id_key('c', design.concave_tier_ids.get(i).copied(), i),
            if design
                .concave_tiers
                .get(i)
                .is_some_and(ConcaveTier::is_crown_side)
            {
                StepSide::Crown
            } else {
                StepSide::Pavilion
            },
            design.tiers.len() + i,
            String::new(),
        ),
    };
    let indices = step_indices(&key, &row.indices);
    let signature = signature_of(design, side, &row, &indices);
    let is_concave = row.concave.is_some();
    let code = row.label().to_owned();
    let instruction = row.instruction();
    CuttingStep {
        number: position + 1,
        key,
        signature,
        tier,
        table_row,
        code,
        name: if row.name.trim().is_empty() {
            "(unnamed)".to_owned()
        } else {
            row.name
        },
        side,
        angle_deg: row.angle_deg,
        indices,
        meet: instruction,
        mast: (!is_concave).then_some(row.mast),
        depth_mm: row.depth_mm,
        cheater_offset_deg: row.cheater_offset_deg,
        notes,
        tool_line: row.concave.as_ref().map(|info| tool_line(design, info)),
    }
}

/// The key of a tier: `prefix` and its stable id, or its position when the design has no id
/// for it (a design edited behind the edit history's back).
fn id_key(prefix: char, id: Option<TierId>, position: usize) -> String {
    id.map_or_else(
        || format!("{prefix}p{position}"),
        |id| format!("{prefix}{}", id.value()),
    )
}

/// The index positions of a step. A tier listing none is a single facet at position 0.
fn step_indices(step_key: &str, listed: &[f64]) -> Vec<StepIndex> {
    let values: &[f64] = if listed.is_empty() { &[0.0] } else { listed };
    values
        .iter()
        .map(|&value| {
            let text = format_index_value(value);
            StepIndex {
                value,
                key: format!("{step_key}#{text}"),
                text,
            }
        })
        .collect()
}

/// A concave tier's tool line: code, azimuth, displacement, size and motion, with the
/// displacement and diameter in millimetres once a girdle diameter anchors a scale.
fn tool_line(design: &Design, info: &ConcaveRowInfo) -> String {
    let [code, theta, displacement, details] = info.second_line_fields();
    let mut line = format!("Tool {code}  ·  azimuth {theta}  ·  {displacement}  ·  {details}");
    if let Some(width_mm) = design.girdle_diameter_mm {
        let [x, y, z] = info.displacement.map(|ratio| ratio * width_mm);
        let _ = write!(
            line,
            "  ·  ({x:.2}, {y:.2}, {z:.2} mm), tool diameter {:.2} mm",
            info.diameter_ratio * width_mm
        );
    }
    line
}

/// The fingerprint of the values a cutter works to: side, angle, the indices (as a set, so
/// listing them in another order changes nothing), mast and depth, the cheater offset and a
/// concave tier's tool. The meet wording and the notes are left out on purpose.
fn signature_of(
    design: &Design,
    side: StepSide,
    row: &CutSheetRow,
    indices: &[StepIndex],
) -> String {
    let mut sorted: Vec<f64> = indices.iter().map(|index| index.value).collect();
    sorted.sort_by(f64::total_cmp);
    let mut text = format!(
        "{}|{}|{}|",
        if row.concave.is_some() {
            "concave"
        } else {
            "flat"
        },
        side.label(),
        fixed(row.angle_deg, 4)
    );
    for value in sorted {
        let _ = write!(text, "{},", fixed(value, 4));
    }
    let _ = write!(
        text,
        "|{}|{}|{}",
        fixed(row.mast, 5),
        row.depth_mm
            .map_or_else(|| "-".to_owned(), |mm| fixed(mm, 4)),
        row.cheater_offset_deg
            .map_or_else(|| "-".to_owned(), |deg| fixed(deg, 4)),
    );
    if let Some(info) = &row.concave {
        for field in info.second_line_fields() {
            text.push('|');
            text.push_str(&field);
        }
        let _ = write!(
            text,
            "|{}",
            design
                .girdle_diameter_mm
                .map_or_else(|| "-".to_owned(), |mm| fixed(mm, 4))
        );
    }
    format!("{:016x}", fnv1a(text.as_bytes()))
}

/// `value` with `precision` decimals and no negative zero, so a mast that comes out as
/// `-0.0` on one run and `0.0` on the next fingerprints the same.
fn fixed(value: f64, precision: usize) -> String {
    let text = format!("{value:.precision$}");
    match text.strip_prefix('-') {
        Some(rest) if rest.bytes().all(|b| b == b'0' || b == b'.') => rest.to_owned(),
        _ => text,
    }
}

/// FNV-1a over `bytes`.
fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(FNV_OFFSET, |hash, &byte| {
        (hash ^ u64::from(byte)).wrapping_mul(FNV_PRIME)
    })
}
