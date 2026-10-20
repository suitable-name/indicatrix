//! The tier list's rows and the status line, built from the shared view models.
//!
//! The rows follow the desktop and web tier tables:
//! - a solve of the current design gives the solved rows;
//! - after an edit, before the next solve, the previous solve's masts are kept (and
//!   marked stale) for every tier the edit did not touch and blanked for the ones it did;
//! - a design that failed to solve gets rows that name the tiers responsible, but only
//!   when it is small enough to solve again on the spot (the shared policy).

use std::time::Duration;

use indicatrix_editor::{
    solve_policy::{auto_solve_off_note, should_schedule_auto_solve},
    view_model::{
        TierRow, TierRowKind,
        row_format::index_chip_items,
        rows::{tier_items, tier_items_from_solved, tier_items_stale_with_last_solved},
    },
};

use super::solve::should_solve_synchronously;
use crate::app_state::AppState;

/// How serious a badge is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BadgeLevel {
    /// Nothing to worry about.
    Good,
    /// Worth a look.
    Check,
    /// Wrong or untrusted.
    Problem,
    /// Neutral information.
    Info,
}

impl BadgeLevel {
    /// The level as the Slint side carries it: 0 good, 1 check, 2 problem, 3 info.
    #[must_use]
    pub const fn index(self) -> i32 {
        match self {
            Self::Good => 0,
            Self::Check => 1,
            Self::Problem => 2,
            Self::Info => 3,
        }
    }
}

/// A short labeled mark on a tier row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Badge {
    /// The text shown.
    pub text: String,
    /// How serious it is.
    pub level: BadgeLevel,
}

impl Badge {
    fn new(text: impl Into<String>, level: BadgeLevel) -> Self {
        Self {
            text: text.into(),
            level,
        }
    }
}

/// The three badges a row can carry.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TierBadges {
    /// How the tier was solved, shown only when that is not the plain answer (a
    /// least-squares estimate, a failure, a missing anchor).
    pub solve: Option<Badge>,
    /// The critical-angle margin of a pavilion tier.
    pub margin: Option<Badge>,
    /// A manufacturability finding.
    pub warning: Option<Badge>,
}

/// One row of the tier list.
#[derive(Debug, Clone, PartialEq, Eq)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "the row mirrors the Slint TierRowData flags; they are independent switches"
)]
pub struct MobileTierRow {
    /// The tier's position in its own list (`design.tiers`, or `design.concave_tiers`
    /// for a concave row).
    pub index: usize,
    /// The tier's code in cutting order (`P1`, `C2`, `T`).
    pub code: String,
    /// The tier's name, or its code when it has none.
    pub name: String,
    /// The angle magnitude, two decimals (the block carries the side).
    pub angle_text: String,
    /// `"Crown"`, `"Pavilion"` or `"Girdle"`.
    pub block: String,
    /// The index list, display-rounded.
    pub indices_text: String,
    /// How many index positions the tier has.
    pub index_count: usize,
    /// The solved mast, four decimals; empty when there is none to show.
    pub mast_text: String,
    /// The mast in millimeters, empty when nothing anchors a real scale.
    pub mast_mm_text: String,
    /// What the tier meets, in words.
    pub constraint_text: String,
    /// The tiers its meet resolved against (`"meets P1, C2"`), may be empty.
    pub meet_partners_text: String,
    /// The solver's label for how the mast was obtained.
    pub strategy_text: String,
    /// The badges.
    pub badges: TierBadges,
    /// The manufacturability findings for this tier, `"; "`-joined.
    pub warning_text: String,
    /// The orbit summary (`"orbit x8"`, `"6/8 orbit"`).
    pub orbit_text: String,
    /// Whether the orbit is incomplete.
    pub orbit_incomplete: bool,
    /// Whether any occurrence is detached from its orbit.
    pub is_detached: bool,
    /// The relation that drives the tier's angle (`"C1 - 4"`), empty when free.
    pub relation_text: String,
    /// What the source file said the tier meets, empty when there is nothing to adopt.
    pub imported_meet_text: String,
    /// Whether a missing scale anchor is this tier's problem.
    pub needs_anchor: bool,
    /// The tier is the single selection.
    pub is_selected: bool,
    /// The tier is in the multi-selection.
    pub is_multi_selected: bool,
    /// The mast shown is from an older solve (or blank): the design changed since.
    pub is_stale: bool,
    /// A concave (tool-cut) tier, edited through its separate typed form.
    pub is_concave: bool,
    /// For a concave tier, its tool line; empty for a flat tier.
    pub tool_line: String,
}

/// Where a row list's masts came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Source {
    Solved,
    Failed,
    Stale,
}

/// The rows of the tier list for the current design: the flat tiers in storage order,
/// then the concave tiers.
#[must_use]
pub fn tier_rows(state: &AppState) -> Vec<MobileTierRow> {
    let design = &state.session.design;
    let generation = state.session.current_generation();
    let n_d = design.effective_refractive_index();
    let solved = state
        .solve
        .current_tiers(generation)
        .filter(|solved| solved.len() == design.tiers.len());
    let (rows, source) = solved.map_or_else(
        || {
            if state.solve.failure(generation).is_some() && should_solve_synchronously(state) {
                (tier_items(design, n_d), Source::Failed)
            } else {
                let dirty = state.solve.dirty_tiers(generation, design.tiers.len());
                (
                    tier_items_stale_with_last_solved(
                        design,
                        n_d,
                        state.solve.last_tiers(),
                        &dirty,
                    ),
                    Source::Stale,
                )
            }
        },
        |solved| (tier_items_from_solved(design, solved, n_d), Source::Solved),
    );
    rows.iter()
        .map(|row| mobile_row(state, row, source))
        .collect()
}

/// One view-model row as a [`MobileTierRow`].
fn mobile_row(state: &AppState, row: &TierRow, source: Source) -> MobileTierRow {
    let concave = row.kind == TierRowKind::Concave;
    let index = usize::try_from(row.index).unwrap_or(0);
    let mast_text = if row.mast == "-" || row.mast == "?" {
        String::new()
    } else {
        row.mast.clone()
    };
    MobileTierRow {
        index,
        code: row.code.clone(),
        name: if row.name.is_empty() {
            row.code.clone()
        } else {
            row.name.clone()
        },
        angle_text: row.angle_deg.clone(),
        block: row.block.clone(),
        indices_text: row.indices.clone(),
        index_count: row
            .indices_full
            .split(',')
            .filter(|part| !part.trim().is_empty())
            .count(),
        mast_text,
        mast_mm_text: row.mast_mm.clone(),
        constraint_text: constraint_summary(row.constraint_kind, &row.constraint_text, concave),
        meet_partners_text: row.meet_partners_text.clone(),
        strategy_text: row.strategy.clone(),
        badges: badges(row, source),
        warning_text: row.warning_text.clone(),
        orbit_text: row.orbit_status.clone(),
        orbit_incomplete: row.orbit_incomplete,
        is_detached: row.is_detached,
        relation_text: row.relation_text.clone(),
        imported_meet_text: row.imported_meet_text.clone(),
        needs_anchor: row.needs_anchor,
        is_selected: if concave {
            state.session.selected_concave == Some(index)
        } else {
            state.selected_tier == Some(index)
        },
        is_multi_selected: !concave && state.session.multi_selected.contains(&index),
        is_stale: source == Source::Stale,
        is_concave: concave,
        tool_line: row.tool_line.clone(),
    }
}

/// What a tier meets, in words, from the view model's kind and text.
fn constraint_summary(kind: i32, text: &str, concave: bool) -> String {
    if concave {
        return "Tool cut".to_owned();
    }
    match kind {
        0 => "Meets existing facets".to_owned(),
        1 => format!("Meets {text}"),
        2 => format!("Anchored at {text}"),
        3 => format!("Cut to depth {text} mm"),
        4 => format!("Girdle thickness {text} mm"),
        5 => format!("Table width {text} mm"),
        _ => String::new(),
    }
}

/// The badges of one row.
fn badges(row: &TierRow, source: Source) -> TierBadges {
    if row.kind == TierRowKind::Concave {
        return TierBadges::default();
    }
    let solve = if row.needs_anchor {
        Some(Badge::new("Needs Anchor", BadgeLevel::Problem))
    } else {
        match source {
            Source::Failed => Some(Badge::new(row.strategy.clone(), BadgeLevel::Problem)),
            Source::Solved if row.strategy_is_uncertain => {
                let level = if row.strategy.starts_with("FAILED") {
                    BadgeLevel::Problem
                } else {
                    BadgeLevel::Check
                };
                Some(Badge::new(row.strategy.clone(), level))
            }
            Source::Stale | Source::Solved => None,
        }
    };
    let margin = (!row.margin_text.is_empty()).then(|| {
        let level = match row.risk_level {
            0 => Some(BadgeLevel::Good),
            1 => Some(BadgeLevel::Check),
            2 => Some(BadgeLevel::Problem),
            _ => None,
        };
        level.map(|level| Badge::new(row.margin_text.clone(), level))
    });
    let warning = (!row.warning_text.is_empty())
        .then(|| Badge::new(row.warning_text.clone(), BadgeLevel::Check));
    TierBadges {
        solve,
        margin: margin.flatten(),
        warning,
    }
}

/// How the status line should be styled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusKind {
    /// Nothing to report (no tiers yet).
    None,
    /// A solve is running.
    Solving,
    /// Solved, no problem.
    Solved,
    /// Solved, but the solid has a problem.
    Problem,
    /// The solve failed.
    Failed,
    /// The design changed since the last solve.
    Stale,
}

/// The status line under the tier list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusLine {
    /// The sentence.
    pub text: String,
    /// Its style.
    pub kind: StatusKind,
}

impl StatusLine {
    /// Whether the text describes a problem (a failed solve or a solid with a problem).
    #[must_use]
    pub const fn is_problem(&self) -> bool {
        matches!(self.kind, StatusKind::Problem | StatusKind::Failed)
    }
}

/// The status line for the design as it is now.
#[must_use]
pub fn status_line(state: &AppState) -> StatusLine {
    let design = &state.session.design;
    let generation = state.session.current_generation();
    if state.solve.is_solving() && !state.solve.is_current(generation) {
        return StatusLine {
            text: "Solving...".to_owned(),
            kind: StatusKind::Solving,
        };
    }
    if design.tiers.is_empty() {
        return StatusLine {
            text: "Preform only. Add tiers to start cutting.".to_owned(),
            kind: StatusKind::None,
        };
    }
    if let Some(solved) = state
        .solve
        .solved()
        .filter(|solved| solved.generation == generation)
    {
        return StatusLine {
            text: format!(
                "{} (solved in {:.2} s)",
                solved.status,
                solved.took.as_secs_f32()
            ),
            kind: if solved.problem {
                StatusKind::Problem
            } else {
                StatusKind::Solved
            },
        };
    }
    if let Some(message) = state.solve.failure(generation) {
        return StatusLine {
            text: message.to_owned(),
            kind: StatusKind::Failed,
        };
    }
    StatusLine {
        text: stale_text(state),
        kind: StatusKind::Stale,
    }
}

/// The sentence for a design that has changed since its last solve.
fn stale_text(state: &AppState) -> String {
    let budget_ms = state.settings.auto_solve_budget_ms;
    if budget_ms == 0 {
        return "Not solved since the last edit. Auto-solve is off: tap Solve.".to_owned();
    }
    let budget = Duration::from_millis(u64::from(budget_ms));
    if !should_solve_synchronously(state)
        && !should_schedule_auto_solve(state.solve.last_took(), budget)
        && let Some(last) = state.solve.last_took()
    {
        return auto_solve_off_note(last);
    }
    "Not solved since the last edit.".to_owned()
}

/// One index position of a tier, for the tier sheet's chips.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexChipView {
    /// The position's display text.
    pub label: String,
    /// Whether this occurrence is detached from its orbit.
    pub detached: bool,
}

/// Everything the tier sheet shows about one flat tier.
#[derive(Debug, Clone, PartialEq, Eq)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "mirrors the tier sheet's Slint properties; the flags are independent switches"
)]
pub struct TierDetail {
    /// The tier's position in `design.tiers`.
    pub index: usize,
    /// The sheet title: the code and, when different, the name.
    pub title: String,
    /// The tier's stored name (may be empty), what the name field edits.
    pub name: String,
    /// The angle magnitude, two decimals.
    pub angle_text: String,
    /// `"Crown"`, `"Pavilion"` or `"Girdle"`.
    pub block: String,
    /// One chip per index position.
    pub indices: Vec<IndexChipView>,
    /// A one-line summary of what the tier meets or is anchored at.
    pub meet_summary: String,
    /// The solver's label and detail for the mast, empty before a solve.
    pub solve_summary: String,
    /// The relation that drives the angle, empty when free.
    pub relation_text: String,
    /// The orbit summary.
    pub orbit_text: String,
    /// Whether the orbit is incomplete (Complete Orbit does something).
    pub orbit_incomplete: bool,
    /// Whether any occurrence is detached (the button then says Reattach).
    pub is_detached: bool,
    /// What the source file said the tier meets, empty when there is nothing to adopt.
    pub imported_meet_text: String,
    /// The manufacturability findings.
    pub warning_text: String,
    /// Whether the tier can move earlier in the cutting order.
    pub can_move_up: bool,
    /// Whether the tier can move later in the cutting order.
    pub can_move_down: bool,
}

/// The detail for the flat tier behind `row`; `None` for a concave row or a tier that no
/// longer exists.
#[must_use]
pub fn tier_detail(state: &AppState, row: &MobileTierRow) -> Option<TierDetail> {
    if row.is_concave {
        return None;
    }
    let design = &state.session.design;
    let tier = design.tiers.get(row.index)?;
    let indices = index_chip_items(&tier.indices, &tier.detached)
        .into_iter()
        .map(|chip| IndexChipView {
            label: chip.label,
            detached: chip.detached,
        })
        .collect();
    let title = if tier.name.trim().is_empty() || row.name == row.code {
        row.code.clone()
    } else {
        format!("{} ({})", row.code, row.name)
    };
    let solve_summary = [row.strategy_text.as_str(), row.mast_text.as_str()]
        .iter()
        .filter(|part| !part.is_empty() && **part != "not solved")
        .copied()
        .collect::<Vec<_>>()
        .join(", ");
    let meet_summary = [
        row.constraint_text.as_str(),
        row.meet_partners_text.as_str(),
    ]
    .iter()
    .filter(|part| !part.is_empty())
    .copied()
    .collect::<Vec<_>>()
    .join(". ");
    Some(TierDetail {
        index: row.index,
        title,
        name: tier.name.clone(),
        angle_text: row.angle_text.clone(),
        block: row.block.clone(),
        indices,
        meet_summary,
        solve_summary,
        relation_text: row.relation_text.clone(),
        orbit_text: row.orbit_text.clone(),
        orbit_incomplete: row.orbit_incomplete,
        is_detached: row.is_detached,
        imported_meet_text: row.imported_meet_text.clone(),
        warning_text: row.warning_text.clone(),
        can_move_up: row.index > 0,
        can_move_down: row.index + 1 < design.tiers.len(),
    })
}
