//! The manufacturability findings behind the tier rows: each finding attributed to the flat
//! tier, the concave tier or the design it is about, and grouped per row.

use super::without_tool_checks;
use indicatrix::geometry::meet_solver::SolvedTier;
use indicatrix_cut_core::{Design, ManufacturabilityWarning};
use std::collections::BTreeMap;

/// [`manufacturability_warnings_tagged`]'s findings split by what they are about:
/// flat tier rows, concave tier rows (each grouped and `"; "`-joined like
/// [`warning_text_by_tier`]), and nothing for the design-wide findings (tool
/// overlaps, slivers, the export notice), which belong to the banner, not a row.
/// One manufacturability pass feeds both, since its mesh checks are the costly part.
pub(super) struct TierWarnings {
    pub(super) flat: BTreeMap<usize, String>,
    pub(super) concave: BTreeMap<usize, String>,
}

impl TierWarnings {
    pub(super) fn of(design: &Design, solved: Option<&[SolvedTier]>) -> Self {
        Self::from_findings(tagged_findings(design, solved))
    }

    /// The same split for findings that were already worked out: `Some` is a full pass a
    /// worker ran against `design` and `solved` (its texts carry no `(pre-solve)` tag).
    /// `None` -- no such pass is available -- runs the pass inline for a design without
    /// concave tools (its mesh checks are cheap, and that is what a caller had before
    /// workers computed it), and for one WITH tools falls back to the mast-free checks
    /// alone, tagged, which never build a mesh.
    pub(super) fn from_precomputed(
        design: &Design,
        solved: &[SolvedTier],
        precomputed: Option<&[ManufacturabilityWarning]>,
    ) -> Self {
        precomputed.map_or_else(
            || Self::of(design, without_tool_checks(design, solved)),
            |warnings| Self::from_findings(findings_of(warnings)),
        )
    }

    fn from_findings(findings: Vec<Finding>) -> Self {
        let (mut flat, mut concave) = (Vec::new(), Vec::new());
        for finding in findings {
            match finding.target {
                WarningTarget::Flat(index) => flat.push((index, finding.text)),
                WarningTarget::Concave(index) => concave.push((index, finding.text)),
                WarningTarget::Design => {}
            }
        }
        Self {
            flat: warning_text_by_tier(&flat),
            concave: warning_text_by_tier(&concave),
        }
    }
}

/// What a manufacturability finding is about, for attributing it to a row.
enum WarningTarget {
    /// A flat tier, by position in `design.tiers`.
    Flat(usize),
    /// A concave tier, by position in `design.concave_tiers`.
    Concave(usize),
    /// The design as a whole (tool overlaps, slivers, the export notice): no row.
    Design,
}

/// One manufacturability finding, attributed.
struct Finding {
    target: WarningTarget,
    text: String,
}

/// [`indicatrix_cut_core::manufacturability::check_manufacturability_available`]'s
/// findings against `design`'s current state, each attributed to the flat tier, the
/// concave tier or the design it is about -- lets a caller badge a row
/// instead of only showing a flattened `String`. `solved` is an already-[`Design::solve`]'d
/// mast list when one is available; passing `None` still runs the two
/// mast-free checks (gear quantization, cut order) -- see
/// [`check_manufacturability_available`](indicatrix_cut_core::manufacturability::check_manufacturability_available)'s
/// own doc comment: a design that has never solved, or no
/// longer does, must not lose every finding, only the two that genuinely need
/// a mesh.
///
/// `ManufacturabilityWarning::tier_index` reads `0` for every concave variant, so
/// attributing by it alone would badge flat tier 0 with a tool's warning; the
/// variants are matched here instead.
fn manufacturability_findings(design: &Design, solved: Option<&[SolvedTier]>) -> Vec<Finding> {
    findings_of(
        &indicatrix_cut_core::manufacturability::check_manufacturability_available(
            design,
            solved,
            indicatrix_cut_core::manufacturability::DEFAULT_MIN_FACET_AREA_FRACTION_OF_W2,
        ),
    )
}

/// Each of `warnings` attributed to the flat tier, the concave tier or the design it is about.
fn findings_of(warnings: &[ManufacturabilityWarning]) -> Vec<Finding> {
    warnings
        .iter()
        .map(|warning| Finding {
            target: match (warning, warning.concave_tier()) {
                (_, Some(tier)) => WarningTarget::Concave(tier),
                (
                    ManufacturabilityWarning::VanishingFacet { .. }
                    | ManufacturabilityWarning::UndersizedFacet { .. }
                    | ManufacturabilityWarning::FractionalIndex { .. }
                    | ManufacturabilityWarning::OutOfOrderMeet { .. }
                    | ManufacturabilityWarning::MeetNameNotAscSafe { .. },
                    None,
                ) => WarningTarget::Flat(warning.tier_index()),
                // `ToolsOverlap`, `ConcaveSliver`, the export notice (and any later
                // variant that names no tier): about the whole design.
                (_, None) => WarningTarget::Design,
            },
            text: warning.to_string(),
        })
        .collect()
}

/// The full manufacturability pass over `design` solved as `solved`.
///
/// What a worker computes so a UI thread can show it without running the mesh checks itself
/// ([`super::tier_items_from_solved_with_warnings`], [`manufacturability_warnings_tagged_with`]).
///
/// The same findings [`manufacturability_warnings_tagged`] and the tier rows would work out
/// from `solved` themselves, in the same order.
///
/// # Panics
///
/// `solved` must have one entry per tier of `design`, as for [`super::tier_items_from_solved`].
#[must_use]
pub fn solved_manufacturability_warnings(
    design: &Design,
    solved: &[SolvedTier],
) -> Vec<ManufacturabilityWarning> {
    indicatrix_cut_core::manufacturability::check_manufacturability_available(
        design,
        Some(solved),
        indicatrix_cut_core::manufacturability::DEFAULT_MIN_FACET_AREA_FRACTION_OF_W2,
    )
}

/// [`manufacturability_findings`] with each text prefixed `"(pre-solve) "` when
/// `solved` is `None` -- see [`manufacturability_warnings_tagged`] for why.
fn tagged_findings(design: &Design, solved: Option<&[SolvedTier]>) -> Vec<Finding> {
    let findings = manufacturability_findings(design, solved);
    if solved.is_some() {
        return findings;
    }
    findings
        .into_iter()
        .map(|finding| Finding {
            text: format!("(pre-solve) {}", finding.text),
            ..finding
        })
        .collect()
}

/// The findings about FLAT tiers, as `(tier_index, display_text)` pairs, each text
/// prefixed `"(pre-solve) "` when `solved` is `None`.
///
/// Every caller that shows these findings without a completed solve backing them must say
/// so: the two mast-free checks are real and actionable before Solve ever runs, but must
/// never be mistaken for a full manufacturability pass once the mesh checks are back in
/// play too. Findings about concave tiers or the design as a whole are not included
/// (they have no flat row to land on); [`super::tier_items`] and its siblings put the concave
/// ones on the concave rows.
#[must_use]
pub fn manufacturability_warnings_tagged(
    design: &Design,
    solved: Option<&[SolvedTier]>,
) -> Vec<(usize, String)> {
    tagged_findings(design, solved)
        .into_iter()
        .filter_map(|finding| match finding.target {
            WarningTarget::Flat(index) => Some((index, finding.text)),
            WarningTarget::Concave(_) | WarningTarget::Design => None,
        })
        .collect()
}

/// [`manufacturability_warnings_tagged`] for a caller on a UI thread.
///
/// The findings about FLAT tiers come from `warnings`, the full pass a worker ran
/// ([`solved_manufacturability_warnings`]), with no mesh built here. `None` -- no pass is
/// available -- gives what
/// [`super::tier_items_from_solved_with_warnings`] shows then: the inline pass over `solved` for a
/// design without concave tools, the mast-free findings tagged `(pre-solve)` for one with.
#[must_use]
pub fn manufacturability_warnings_tagged_with(
    design: &Design,
    solved: &[SolvedTier],
    warnings: Option<&[ManufacturabilityWarning]>,
) -> Vec<(usize, String)> {
    let Some(warnings) = warnings else {
        return manufacturability_warnings_tagged(design, without_tool_checks(design, solved));
    };
    findings_of(warnings)
        .into_iter()
        .filter_map(|finding| match finding.target {
            WarningTarget::Flat(index) => Some((index, finding.text)),
            WarningTarget::Concave(_) | WarningTarget::Design => None,
        })
        .collect()
}

/// Groups [`manufacturability_warnings_tagged`]'s
/// pairs by tier index, joining more than one finding for the same tier with
/// `"; "` -- what [`super::tier_items`]/[`super::tier_items_stale`] feed into each row's
/// [`TierRow::warning_text`](crate::view_model::TierRow::warning_text).
fn warning_text_by_tier(pairs: &[(usize, String)]) -> BTreeMap<usize, String> {
    let mut grouped: BTreeMap<usize, Vec<String>> = BTreeMap::new();
    for (tier_index, text) in pairs {
        grouped.entry(*tier_index).or_default().push(text.clone());
    }
    grouped
        .into_iter()
        .map(|(index, texts)| (index, texts.join("; ")))
        .collect()
}
