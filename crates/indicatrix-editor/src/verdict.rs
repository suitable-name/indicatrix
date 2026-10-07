//! The overall verdict: one green / amber / red answer for the open design, the reasons
//! behind it, and a "fix it" action for each reason where a safe tool exists.
//!
//! Three levels ([`Level`]): **Good** (the stone solves, closes, and nothing needs a second
//! look), **Check** (it can be cut, but something deserves attention) and **Problem** (it does
//! not solve or close, or the optics are clearly poor). [`evaluate`] turns plain
//! [`VerdictInputs`] into a [`Verdict`] with a one-line headline and a list of [`Reason`]s;
//! it is pure, so it is tested on constructed inputs without a window, a solve or a clock.
//!
//! - [`gather()`] reads the inputs off a design and the solve that goes with it (no extra
//!   solve: the caller hands in the mast list it already has).
//! - [`plan_fix`] turns a reason's [`FixAction`] into one [`Edit`], validated against the
//!   same gate Retarget uses (`retarget::validity`), so a fix never trades one problem for a
//!   worse one. It solves the design (and a candidate) itself, so a caller runs it off the UI
//!   thread. The caller applies the edit through the session, which makes it ONE undo step,
//!   and re-solves; the verdict is recomputed from that solve.
//!
//! The optical figures (windowing, extinction, brilliance with the table up) take a few
//! milliseconds to measure and so are an OPTIONAL input: a caller measures them off the UI
//! thread and re-evaluates when they arrive.
//!
//! # Thresholds
//!
//! The optical thresholds below are named constants. Windowing follows the brief (above 15 %
//! is a Check, above 30 % a Problem: at 30 % a third of the stone is a see-through window).
//! Extinction and brilliance are set loosely, so that only a stone that is clearly off is
//! flagged; they are starting points to tune against real designs once the readouts of a
//! few known-good stones are at hand, not measured limits.

mod fix;
mod gather;
#[cfg(test)]
mod tests;

use crate::retarget::metrics::MetricColumn;
use indicatrix_cut_core::ManufacturabilityWarning;
use std::{
    cmp::Reverse,
    collections::{BTreeMap, BTreeSet},
};

pub use fix::{FixPlan, QUICK_ADD_TABLE_MAST, plan_fix, steep_target_deg};
pub use gather::gather;

/// Windowing above this many percent of the stone is worth a look.
///
/// Light that leaks straight out of the pavilion is lost to the eye, so a stone that shows it
/// over a seventh of its face is worth a second look.
pub const WINDOWING_CHECK_PCT: f32 = 15.0;

/// Windowing above this many percent is a real problem: a third of the stone is a window.
pub const WINDOWING_PROBLEM_PCT: f32 = 30.0;

/// Extinction above this many percent is worth a look: two fifths of the light trapped or
/// lost is a stone that reads dark. Set loosely (see the module docs).
pub const EXTINCTION_CHECK_PCT: f32 = 40.0;

/// Extinction above this many percent is a real problem (three fifths of the light lost).
pub const EXTINCTION_PROBLEM_PCT: f32 = 60.0;

/// Brilliance (light returned to the eye, table up) below this many percent is worth a look:
/// a quarter of the light returned or less is a dull stone. Set loosely (see the module docs).
pub const BRILLIANCE_CHECK_BELOW_PCT: f32 = 25.0;

/// Brilliance below this many percent is a real problem (one tenth of the light returned).
pub const BRILLIANCE_PROBLEM_BELOW_PCT: f32 = 10.0;

/// A steepened pavilion facet ends this many degrees above the critical angle.
///
/// Two degrees is the lower edge of `Risk::Safe` in `indicatrix_cut_core::optics_hints`, so the
/// tier table stops badging the facet once it is fixed.
pub const SAFE_MARGIN_DEG: f64 = 2.0;

/// At most this many pavilion tiers are listed one by one; the rest are summed up in one line.
pub const MAX_LISTED_WINDOWING_TIERS: usize = 4;

/// How good the open design is, from best to worst.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Level {
    /// Nothing needs a second look.
    Good,
    /// It can be cut, but something deserves attention.
    Check,
    /// It does not solve or close, or the optics are clearly poor.
    Problem,
}

impl Level {
    /// The one word the badge shows.
    #[must_use]
    pub const fn word(self) -> &'static str {
        match self {
            Self::Good => "Good",
            Self::Check => "Check",
            Self::Problem => "Problem",
        }
    }

    /// The level as the small integer the Slint side carries (`0` good, `1` check, `2`
    /// problem).
    #[must_use]
    pub const fn code(self) -> i32 {
        match self {
            Self::Good => 0,
            Self::Check => 1,
            Self::Problem => 2,
        }
    }
}

/// What a [`Reason`] is about, for the code that has to tell them apart (the headline, the
/// tests); a cutter reads [`Reason::text`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReasonKind {
    /// The design does not solve.
    DoesNotSolve,
    /// It solves, but the facets do not enclose a stone.
    NotClosed,
    /// A tier has facets that later tiers cut away.
    Vanished,
    /// A tier has facets too small to polish.
    Undersized,
    /// A tier has index positions between gear teeth.
    OffGear,
    /// A tier meets a tier that is cut later.
    OutOfOrder,
    /// A meet name would not survive saving as plain `.asc` text.
    NameNotSafe,
    /// A concave tool does something it should not.
    Concave,
    /// A pavilion facet is below the critical angle.
    Windowing,
    /// The crown has no table.
    MissingTable,
    /// A proportion is outside the usual range.
    Proportion,
    /// Too much of the stone windows (measured).
    OpticalWindowing,
    /// Too much light is lost (measured).
    OpticalExtinction,
    /// Too little light returns to the eye (measured).
    OpticalBrilliance,
}

/// A change that can fix a [`Reason`]. Each is planned by [`plan_fix`] and applied as one
/// undo step.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FixAction {
    /// Move every index position of the tier to the nearest gear tooth.
    SnapToTeeth {
        /// The tier's position in `design.tiers`.
        tier: usize,
    },
    /// Remove the facets of the tier that later tiers cut away (the whole tier when all of
    /// them are).
    RemoveVanished {
        /// The tier's position in `design.tiers`.
        tier: usize,
    },
    /// Move the tier to just after the latest tier it meets.
    MoveAfter {
        /// The tier's position in `design.tiers`.
        tier: usize,
        /// The position of the tier it must come after.
        after: usize,
    },
    /// Steepen the pavilion tier to the critical angle plus [`SAFE_MARGIN_DEG`], turning it
    /// about its girdle edge so the girdle stays.
    SteepenPavilion {
        /// The tier's position in `design.tiers`.
        tier: usize,
    },
    /// Add a flat table facet to the crown.
    AddTable,
}

impl FixAction {
    /// The button's label.
    #[must_use]
    pub const fn label(&self) -> &'static str {
        match self {
            Self::SnapToTeeth { .. } => "Snap to teeth",
            Self::RemoveVanished { .. } => "Remove",
            Self::MoveAfter { .. } => "Move later",
            Self::SteepenPavilion { .. } => "Steepen",
            Self::AddTable => "Add a table",
        }
    }

    /// What the button does, for its tooltip.
    #[must_use]
    pub const fn hint(&self) -> &'static str {
        match self {
            Self::SnapToTeeth { .. } => {
                "Moves each index position of this tier to the nearest gear tooth."
            }
            Self::RemoveVanished { .. } => {
                "Removes the facets that are cut away. One undo brings them back."
            }
            Self::MoveAfter { .. } => {
                "Moves this tier to just after the tier it meets, so that tier is cut first."
            }
            Self::SteepenPavilion { .. } => {
                "Raises this facet to the critical angle plus 2 degrees. It turns about its \
                 girdle edge, so the girdle stays where it is."
            }
            Self::AddTable => {
                "Adds a flat table facet at a height that keeps all the crown facets."
            }
        }
    }
}

/// One thing the verdict found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reason {
    /// What it is about.
    pub kind: ReasonKind,
    /// How serious it is ([`Level::Check`] or [`Level::Problem`]).
    pub level: Level,
    /// One plain sentence.
    pub text: String,
    /// The tier to select for "Show", a position in `design.tiers`.
    pub tier: Option<usize>,
    /// The fix, where a safe tool exists.
    pub fix: Option<FixAction>,
    /// A question to ask before the fix runs (a removal), `None` when it needs none.
    pub confirm: Option<String>,
}

/// The overall answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Verdict {
    /// The worst level among the reasons ([`Level::Good`] without any).
    pub level: Level,
    /// One short sentence: the badge's tooltip.
    pub headline: String,
    /// Every reason, the worst first.
    pub reasons: Vec<Reason>,
}

/// Whether, and how well, the design solves.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum SolveFacts {
    /// The design has no tiers yet: there is nothing to judge.
    #[default]
    Empty,
    /// It does not solve; the sentence says why.
    DoesNotSolve(String),
    /// It solves, but the facets do not enclose a stone; the sentence says what is wrong.
    NotClosed(String),
    /// It solves and closes.
    Closed,
}

/// A tier whose facets let light out: a pavilion tier below the critical angle for the
/// design's material, or a crown tier the crown-aware estimate says leaks through the crown.
#[derive(Debug, Clone, PartialEq)]
pub struct TierWindowing {
    /// The tier's position in `design.tiers`.
    pub tier: usize,
    /// The tier's name (may be empty).
    pub name: String,
    /// The tier's angle from the horizontal, degrees, as a magnitude.
    pub angle_deg: f64,
    /// The critical angle for the material, degrees.
    pub critical_deg: f64,
    /// The margin past the critical angle (negative here): `angle_deg - critical_deg` for a
    /// pavilion tier, the crown-window margin for a crown tier.
    pub margin_deg: f64,
    /// `true` for a crown tier, whose margin is the crown-aware ESTIMATE (it follows one ray
    /// through the crown facet onto the design's main pavilion facet on the same side, so it
    /// assumes the design's main pavilion angle and nothing else about the stone). The reason
    /// text says so. No fix is offered for those.
    pub crown_estimate: bool,
    /// Whether [`FixAction::SteepenPavilion`] is offered for it (a pavilion tier that no
    /// relation drives and that can reach the safe angle).
    pub fixable: bool,
}

/// One proportion judged against the reference windows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProportionFact {
    /// What was judged, as the start of a sentence ("The table width").
    pub label: &'static str,
    /// `0` within, `1` near, `2` outside, `-1` nothing to judge.
    pub level: i32,
    /// The reference window's explanation.
    pub reason: String,
}

/// Everything [`evaluate`] reads.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct VerdictInputs {
    /// Whether the design solves and closes.
    pub solve: SolveFacts,
    /// The manufacturability findings (the mast-free ones only while the design does not
    /// solve).
    pub warnings: Vec<ManufacturabilityWarning>,
    /// Tiers where only some facets vanish and the vanished ones can be told apart, so
    /// [`FixAction::RemoveVanished`] can drop just those.
    pub partial_vanish_fixable: BTreeSet<usize>,
    /// Pavilion tiers below the critical angle.
    pub windowing: Vec<TierWindowing>,
    /// The proportions judged (empty unless the design is a round brilliant family).
    pub proportions: Vec<ProportionFact>,
    /// The crown has facets but no table.
    pub missing_table: bool,
    /// The measured table-up optics, when they are known.
    pub optics: Option<MetricColumn>,
    /// What a person reads for every tier, in schedule order: the tier's own name, or its
    /// standard code (`P2`, `C1`) when the name is empty or old-style (`3`, `A`)
    /// ([`crate::retarget::plan::tier_display_names`]). The reasons call a tier by this. Empty
    /// (the default) falls back to the name each finding carries.
    pub tier_names: Vec<String>,
}

impl VerdictInputs {
    /// What the reasons call tier number `tier` (0-based) whose own name is `name`: the
    /// display name in [`Self::tier_names`] when there is one, otherwise [`label_of`].
    fn label(&self, name: &str, tier: usize) -> String {
        self.tier_names
            .get(tier)
            .filter(|shown| !shown.trim().is_empty())
            .map_or_else(|| label_of(name, tier), Clone::clone)
    }

    /// These inputs with `optics` filled in (the measurement arrived after the rest).
    #[must_use]
    pub fn with_optics(&self, optics: Option<MetricColumn>) -> Self {
        Self {
            optics,
            ..self.clone()
        }
    }

    /// `true` when there is nothing to judge (the design has no tiers).
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        matches!(self.solve, SolveFacts::Empty)
    }
}

/// The tier's name, or "Tier N" for an unnamed one.
fn label_of(name: &str, tier: usize) -> String {
    if name.trim().is_empty() {
        format!("Tier {}", tier + 1)
    } else {
        name.to_string()
    }
}

/// `text` as one sentence: trimmed, with a closing full stop.
fn sentence(text: &str) -> String {
    let trimmed = text.trim();
    if trimmed.is_empty() || trimmed.ends_with(['.', '!', '?']) {
        trimmed.to_string()
    } else {
        format!("{trimmed}.")
    }
}

const fn plural(count: usize) -> &'static str {
    if count == 1 { "" } else { "s" }
}

const fn reason(kind: ReasonKind, level: Level, text: String) -> Reason {
    Reason {
        kind,
        level,
        text,
        tier: None,
        fix: None,
        confirm: None,
    }
}

/// The reasons the structural findings give: one per vanished tier.
fn vanished_reasons(inputs: &VerdictInputs) -> Vec<Reason> {
    inputs
        .warnings
        .iter()
        .filter_map(|warning| {
            let ManufacturabilityWarning::VanishingFacet {
                tier_index,
                tier_name,
                vanished,
                total,
                ..
            } = warning
            else {
                return None;
            };
            let label = inputs.label(tier_name, *tier_index);
            let all_gone = vanished >= total;
            let text = if all_gone && *total == 1 {
                format!("{label}: its facet is cut away by a later tier.")
            } else if all_gone {
                format!("{label}: all {total} of its facets are cut away by later tiers.")
            } else {
                format!("{label}: {vanished} of {total} facets are cut away by later tiers.")
            };
            let fixable = all_gone || inputs.partial_vanish_fixable.contains(tier_index);
            let mut found = reason(ReasonKind::Vanished, Level::Check, text);
            found.tier = Some(*tier_index);
            if fixable {
                found.fix = Some(FixAction::RemoveVanished { tier: *tier_index });
                found.confirm = Some(if all_gone {
                    format!("Remove {label} from the schedule? It adds nothing to the stone.")
                } else {
                    format!(
                        "Remove the {vanished} cut-away facet{} of {label}? The other {} stay.",
                        plural(*vanished),
                        total - vanished
                    )
                });
            }
            Some(found)
        })
        .collect()
}

/// The reasons for facets that are too small, one per tier.
fn undersized_reasons(inputs: &VerdictInputs) -> Vec<Reason> {
    // tier -> (name, how many, smallest width as percent of the stone)
    let mut groups: BTreeMap<usize, (String, usize, f64)> = BTreeMap::new();
    for warning in &inputs.warnings {
        if let ManufacturabilityWarning::UndersizedFacet {
            tier_index,
            tier_name,
            area,
            width_axis,
            ..
        } = warning
        {
            let percent = if *width_axis > 0.0 {
                100.0 * area.sqrt() / width_axis
            } else {
                f64::NAN
            };
            let entry = groups
                .entry(*tier_index)
                .or_insert_with(|| (tier_name.clone(), 0, f64::INFINITY));
            entry.1 += 1;
            if percent.is_finite() {
                entry.2 = entry.2.min(percent);
            }
        }
    }
    groups
        .into_iter()
        .map(|(tier, (name, count, smallest))| {
            let label = inputs.label(&name, tier);
            let text = match (count, smallest.is_finite()) {
                (1, true) => format!(
                    "{label}: a facet is only {smallest:.1} % of the stone's width, too small to polish."
                ),
                (1, false) => format!("{label}: a facet is too small to polish."),
                (_, true) => format!(
                    "{label}: {count} facets are too small to polish (the smallest is {smallest:.1} % of the stone's width)."
                ),
                (_, false) => format!("{label}: {count} facets are too small to polish."),
            };
            let mut found = reason(ReasonKind::Undersized, Level::Check, text);
            found.tier = Some(tier);
            found
        })
        .collect()
}

/// The reasons for index positions between gear teeth, one per tier.
fn off_gear_reasons(inputs: &VerdictInputs) -> Vec<Reason> {
    // tier -> (name, how many, the first nearest tooth)
    let mut groups: BTreeMap<usize, (String, usize, f64)> = BTreeMap::new();
    for warning in &inputs.warnings {
        if let ManufacturabilityWarning::FractionalIndex {
            tier_index,
            tier_name,
            achievable,
            ..
        } = warning
        {
            let entry = groups
                .entry(*tier_index)
                .or_insert_with(|| (tier_name.clone(), 0, *achievable));
            entry.1 += 1;
        }
    }
    groups
        .into_iter()
        .map(|(tier, (name, count, tooth))| {
            let label = inputs.label(&name, tier);
            let text = if count == 1 {
                format!(
                    "{label}: an index position sits between gear teeth (the nearest tooth is {tooth})."
                )
            } else {
                format!("{label}: {count} index positions sit between gear teeth.")
            };
            let mut found = reason(ReasonKind::OffGear, Level::Check, text);
            found.tier = Some(tier);
            found.fix = Some(FixAction::SnapToTeeth { tier });
            found
        })
        .collect()
}

/// The reasons for a tier that meets a tier cut later, one per tier.
fn out_of_order_reasons(inputs: &VerdictInputs) -> Vec<Reason> {
    // tier -> (name, how many, the latest target and its name)
    let mut groups: BTreeMap<usize, (String, usize, usize, String)> = BTreeMap::new();
    for warning in &inputs.warnings {
        if let ManufacturabilityWarning::OutOfOrderMeet {
            tier_index,
            tier_name,
            target_tier_index,
            target_tier_name,
            ..
        } = warning
        {
            let entry = groups.entry(*tier_index).or_insert_with(|| {
                (
                    tier_name.clone(),
                    0,
                    *target_tier_index,
                    target_tier_name.clone(),
                )
            });
            entry.1 += 1;
            if *target_tier_index > entry.2 {
                entry.2 = *target_tier_index;
                entry.3.clone_from(target_tier_name);
            }
        }
    }
    groups
        .into_iter()
        .map(|(tier, (name, count, latest, latest_name))| {
            let label = inputs.label(&name, tier);
            let target = inputs.label(&latest_name, latest);
            let text = if count == 1 {
                format!("{label} meets {target}, which is cut later.")
            } else {
                format!("{label} meets {count} tiers that are cut later (the last is {target}).")
            };
            let mut found = reason(ReasonKind::OutOfOrder, Level::Check, text);
            found.tier = Some(tier);
            // A tier that names itself cannot be moved after itself.
            if latest > tier {
                found.fix = Some(FixAction::MoveAfter {
                    tier,
                    after: latest,
                });
            }
            found
        })
        .collect()
}

/// The reasons for the findings that have no fix: unsafe meet names and concave tools.
fn other_warning_reasons(inputs: &VerdictInputs) -> Vec<Reason> {
    inputs
        .warnings
        .iter()
        .filter_map(|warning| match warning {
            ManufacturabilityWarning::MeetNameNotAscSafe {
                tier_index,
                tier_name,
                unsafe_names,
                ..
            } => {
                let label = inputs.label(tier_name, *tier_index);
                let mut found = reason(
                    ReasonKind::NameNotSafe,
                    Level::Check,
                    format!(
                        "{label}: the meet name(s) {} would not survive saving as plain .asc text. \
                         Rename them without spaces, commas or semicolons.",
                        unsafe_names.join(", ")
                    ),
                );
                found.tier = Some(*tier_index);
                Some(found)
            }
            // About the export, not the stone.
            ManufacturabilityWarning::ConcaveTiersOmittedFromExport { .. }
            | ManufacturabilityWarning::VanishingFacet { .. }
            | ManufacturabilityWarning::UndersizedFacet { .. }
            | ManufacturabilityWarning::FractionalIndex { .. }
            | ManufacturabilityWarning::OutOfOrderMeet { .. } => None,
            ManufacturabilityWarning::ToolMissesStone { .. }
            | ManufacturabilityWarning::ToolBreaksThrough { .. }
            | ManufacturabilityWarning::ToolRemovesMeet { .. }
            | ManufacturabilityWarning::ToolsOverlap { .. }
            | ManufacturabilityWarning::ConcaveSliver { .. }
            | ManufacturabilityWarning::ToolRemovesHullVertex { .. }
            | ManufacturabilityWarning::ToolEnclosed { .. } => Some(reason(
                ReasonKind::Concave,
                Level::Check,
                sentence(&capitalised(&warning.to_string())),
            )),
        })
        .collect()
}

/// The reasons for facets that let light out, pavilion tiers first and the worst margin
/// first within each kind, capped.
fn windowing_reasons(inputs: &VerdictInputs) -> Vec<Reason> {
    let mut tiers: Vec<&TierWindowing> = inputs.windowing.iter().collect();
    tiers.sort_by(|a, b| {
        a.crown_estimate
            .cmp(&b.crown_estimate)
            .then_with(|| a.margin_deg.total_cmp(&b.margin_deg))
    });
    let mut reasons: Vec<Reason> = tiers
        .iter()
        .take(MAX_LISTED_WINDOWING_TIERS)
        .map(|tier| {
            let label = inputs.label(&tier.name, tier.tier);
            let text = if tier.crown_estimate {
                format!(
                    "{label}: estimated {:+.1}\u{b0} from the critical angle, so some light may leak out through the crown. The estimate follows one ray through this facet and the main pavilion facet on the same side, not the whole stone.",
                    tier.margin_deg
                )
            } else {
                format!(
                    "{label}: at {:.1}\u{b0} it is below the critical angle ({:.1}\u{b0} for this material), so light leaks out of the bottom.",
                    tier.angle_deg, tier.critical_deg
                )
            };
            let mut found = reason(ReasonKind::Windowing, Level::Check, text);
            found.tier = Some(tier.tier);
            if tier.fixable {
                found.fix = Some(FixAction::SteepenPavilion { tier: tier.tier });
            }
            found
        })
        .collect();
    let more = tiers.len().saturating_sub(MAX_LISTED_WINDOWING_TIERS);
    if more > 0 {
        reasons.push(reason(
            ReasonKind::Windowing,
            Level::Check,
            format!(
                "{more} more tier{} let{} light out; the tier table shows which.",
                plural(more),
                if more == 1 { "s" } else { "" }
            ),
        ));
    }
    reasons
}

/// The reasons the measured optics give.
fn optics_reasons(optics: &MetricColumn) -> Vec<Reason> {
    let mut reasons = Vec::new();
    let windowing = optics.windowing_pct;
    if windowing > WINDOWING_CHECK_PCT {
        let level = if windowing > WINDOWING_PROBLEM_PCT {
            Level::Problem
        } else {
            Level::Check
        };
        reasons.push(reason(
            ReasonKind::OpticalWindowing,
            level,
            format!(
                "About {windowing:.0} % of the stone leaks light straight out of the bottom (windowing)."
            ),
        ));
    }
    let extinction = optics.extinction_pct;
    if extinction > EXTINCTION_CHECK_PCT {
        let level = if extinction > EXTINCTION_PROBLEM_PCT {
            Level::Problem
        } else {
            Level::Check
        };
        reasons.push(reason(
            ReasonKind::OpticalExtinction,
            level,
            format!("About {extinction:.0} % of the light is lost inside the stone (extinction)."),
        ));
    }
    let brilliance = optics.brilliance_pct;
    if brilliance < BRILLIANCE_CHECK_BELOW_PCT {
        let level = if brilliance < BRILLIANCE_PROBLEM_BELOW_PCT {
            Level::Problem
        } else {
            Level::Check
        };
        reasons.push(reason(
            ReasonKind::OpticalBrilliance,
            level,
            format!(
                "Only about {brilliance:.0} % of the light returns to the eye with the table up."
            ),
        ));
    }
    reasons
}

/// The headline for `level` and the sorted `reasons`.
fn headline_for(level: Level, reasons: &[Reason], with_optics: bool) -> String {
    match level {
        Level::Good if with_optics => "Looks good: closes, no warnings, little windowing.".into(),
        Level::Good => "Looks good: closes, no warnings.".into(),
        Level::Check => format!("Check {} thing{}.", reasons.len(), plural(reasons.len())),
        Level::Problem => {
            if let Some(first) = reasons
                .first()
                .filter(|r| matches!(r.kind, ReasonKind::DoesNotSolve | ReasonKind::NotClosed))
            {
                return first.text.clone();
            }
            let problems = reasons.iter().filter(|r| r.level == Level::Problem).count();
            let checks = reasons.len() - problems;
            if checks > 0 {
                format!(
                    "{problems} problem{} to fix, {checks} more thing{} to check.",
                    plural(problems),
                    plural(checks)
                )
            } else {
                format!("{problems} problem{} to fix.", plural(problems))
            }
        }
    }
}

/// `text` with its first letter in capitals (the concave findings start in lower case).
fn capitalised(text: &str) -> String {
    let mut chars = text.chars();
    chars.next().map_or_else(String::new, |first| {
        first.to_uppercase().chain(chars).collect()
    })
}

/// The verdict for `inputs`.
///
/// Reasons are ordered worst first, and in a fixed order within a level: solving, closing,
/// vanished facets, small facets, off-gear indices, cut order, names, concave tools,
/// windowing, the table, proportions, then the measured optics.
#[must_use]
pub fn evaluate(inputs: &VerdictInputs) -> Verdict {
    if inputs.is_empty() {
        return Verdict {
            level: Level::Good,
            headline: "Nothing to check yet: add some tiers.".into(),
            reasons: Vec::new(),
        };
    }
    let mut reasons = Vec::new();
    match &inputs.solve {
        SolveFacts::DoesNotSolve(why) => reasons.push(reason(
            ReasonKind::DoesNotSolve,
            Level::Problem,
            format!("Does not solve: {}", sentence(why)),
        )),
        SolveFacts::NotClosed(why) => reasons.push(reason(
            ReasonKind::NotClosed,
            Level::Problem,
            format!("Does not close: {}", sentence(why)),
        )),
        SolveFacts::Empty | SolveFacts::Closed => {}
    }
    reasons.extend(vanished_reasons(inputs));
    reasons.extend(undersized_reasons(inputs));
    reasons.extend(off_gear_reasons(inputs));
    reasons.extend(out_of_order_reasons(inputs));
    reasons.extend(other_warning_reasons(inputs));
    reasons.extend(windowing_reasons(inputs));
    if inputs.missing_table {
        let mut found = reason(
            ReasonKind::MissingTable,
            Level::Check,
            "The crown has no table: nothing is cut flat across the top.".into(),
        );
        found.fix = Some(FixAction::AddTable);
        reasons.push(found);
    }
    for fact in inputs.proportions.iter().filter(|fact| fact.level >= 2) {
        reasons.push(reason(
            ReasonKind::Proportion,
            Level::Check,
            format!(
                "{} is outside the usual range: {}",
                fact.label,
                sentence(&fact.reason)
            ),
        ));
    }
    if let Some(optics) = &inputs.optics {
        reasons.extend(optics_reasons(optics));
    }
    // Stable: the order above survives within a level.
    reasons.sort_by_key(|r| Reverse(r.level));
    let level = reasons.first().map_or(Level::Good, |r| r.level);
    let headline = headline_for(level, &reasons, inputs.optics.is_some());
    Verdict {
        level,
        headline,
        reasons,
    }
}
