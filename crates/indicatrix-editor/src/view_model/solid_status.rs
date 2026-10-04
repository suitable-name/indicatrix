//! The validation-banner text.
//!
//! [`status_text_and_is_problem`]/ [`status_text_and_is_problem_from_solved`] turn
//! [`Design::status`] (or an already-solved mast list) into the Edit tab's one-sentence
//! verdict, and [`design_to_gpu_planes`]/[`design_to_gpu_geometry`]/[`tier_matches_filter`] are the small,
//! unrelated view-model helpers left with no better home once the rest of `state/mod.rs`
//! was split by topic.

use super::row_format::tier_label;
use crate::solve_policy;
use indicatrix::geometry::{
    GpuFacetPlane, ToolPrimitive,
    meet_solver::SolvedTier,
    stone_metrics::{SolidMetrics, SolidStatus, build_solid_mesh, measure_solid},
};
use indicatrix_cut_core::{Design, degenerate_suspects, design::ToolPlacements};

/// [`SolidStatus::Unbounded`]'s escaping
/// plane indices, named by the tier that contributed each one
/// ([`Design::tier_for_plane_index`]) instead of shown as raw indices into the
/// combined plane arrangement. Falls back to `"plane <n>"` for a preform plane
/// or an index [`Design::tier_for_plane_index`] can't place -- both mean "not
/// a schedule-tier facet," not a bug worth panicking over here.
fn escaping_tier_text(design: &Design, solved: &[SolvedTier], escaping: &[usize]) -> String {
    escaping
        .iter()
        .map(|&plane_index| {
            design
                .tier_for_plane_index(solved, plane_index)
                .map_or_else(
                    || format!("plane {plane_index}"),
                    |tier_index| tier_label(design, tier_index),
                )
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// [`SolidStatus::Degenerate`]'s suspect
/// tiers ([`indicatrix_cut_core::degenerate_suspects`]) as a trailing clause,
/// e.g. `" -- check tier 5 (Girdle), tier 8"` -- `""` when nothing is
/// suspect (every tier's mast came from a real anchor or real vertex-derived
/// structure, so a degenerate result has no single tier more likely at fault
/// than another).
fn degenerate_suspects_note(design: &Design, solved: &[SolvedTier]) -> String {
    let suspects = degenerate_suspects(solved);
    if suspects.is_empty() {
        return String::new();
    }
    let names: Vec<String> = suspects
        .into_iter()
        .map(|index| tier_label(design, index))
        .collect();
    format!(" -- check {}", names.join(", "))
}

/// GemCad/GemCutStudio-style external proportion ratios (`L/W`, `H/W`, and
/// `C/W`/`P/W` where a live girdle facet exists) computed from `m`, so the
/// validation banner reports figures a cutter can compare against
/// GemCad/GCS's own recalculation output, not just a bare cubic-model-unit
/// volume that is meaningless next to what GemCad/GCS show.
/// The ratios are dimensionless, so no unit suffix is needed regardless of
/// whether the design has a real millimetre scale (unlike
/// `super::yield_report::proportions_texts`, which is -- these two intentionally
/// never share a formatter, since the banner's ratios and the Preform tab's
/// absolute lengths answer different questions). `""` when the solid measures
/// zero width (never happens for a real `Closed` solid, but avoids a division
/// by zero).
///
/// `mm_per_unit` -- [`Design::yield_report`]'s own scale factor, `Some` only
/// when a girdle diameter is set and the design measures -- appends the finished
/// stone's absolute width x total-depth in millimetres alongside the ratios
/// above, the other half of what GemCad/GCS report after every recalculation
/// (the ratios alone still leave "how big is it really" to the Yield section).
/// `None` (no girdle diameter set yet) omits this clause entirely rather than
/// showing a guessed or model-unit figure next to genuine millimetres.
pub(super) fn external_proportions_note(m: &SolidMetrics, mm_per_unit: Option<f64>) -> String {
    if m.width_axis <= 0.0 {
        return String::new();
    }
    let mut parts = vec![
        format!("L/W {:.3}", m.length_axis / m.width_axis),
        format!("H/W {:.3}", m.total_height / m.width_axis),
    ];
    if let Some(c) = m.crown_height {
        parts.push(format!("C/W {:.3}", c / m.width_axis));
    }
    if let Some(p) = m.pavilion_depth {
        parts.push(format!("P/W {:.3}", p / m.width_axis));
    }
    if let Some(mm) = mm_per_unit {
        parts.push(format!(
            "{:.2} x {:.2} mm",
            m.width_axis * mm,
            m.total_height * mm
        ));
    }
    format!(", {}", parts.join(", "))
}

/// [`Design::status`], rendered as the Edit tab's validation banner text plus whether
/// it should be styled as a problem (red) or all-clear (green).
///
/// Every branch reads as
/// one clean sentence (or, for a multi-block [`indicatrix_cut_core::MissingAnchor`],
/// one full sentence per block) -- no stray leading/trailing punctuation left over
/// from wrapping a nested message in another sentence.
///
/// `Unbounded`/`Degenerate` both name the actual tier(s) responsible
/// ([`escaping_tier_text`]/[`degenerate_suspects_note`]) rather than raw plane indices
/// or nothing at all -- see those functions' own doc comments. `Unbounded` is
/// "essentially unreachable" through `Design` in practice (the preform always caps
/// every direction), not provably impossible, so it still gets a real message rather
/// than being treated as dead code. Either can ALSO be
/// [`solve_policy::too_many_planes_message`]'s plane-cap sentence instead -- see that
/// function's own doc comment.
pub fn status_text_and_is_problem(design: &Design) -> (String, bool) {
    // A tierless design (a bare, uncut preform) solves trivially -- an
    // empty mast list is a valid, closed, zero-plane solve -- so `status()`
    // below would otherwise report it `Closed` and print the PREFORM's own
    // L/W, H/W (`external_proportions_note`) as though they belonged to a
    // finished stone. Apply the SAME guard the Preform tab's own proportions
    // readouts already use (`state::yield_report::proportions_texts`'s own
    // doc comment) before ever calling `status()`.
    if design.tiers.is_empty() {
        return ("Preform only -- add tiers.".to_string(), false);
    }
    match design.status() {
        Ok(SolidStatus::Closed(_)) => {
            // `design.measure()` re-solves and re-meshes independently of `status()`
            // above (no caching), so it can in principle disagree about closure;
            // `.flatten()` treats that theoretical `Ok(None)` the same as an error --
            // either way there's no volume figure to show.
            let volume_note = design
                .measure()
                .ok()
                .flatten()
                .map(|m| {
                    // A second, independent `solve()` (same "no caching" reasoning
                    // as `measure()`'s own doc comment above) purely to read
                    // `yield_report`'s scale factor -- `status_text_and_is_problem_
                    // from_solved` below avoids this because it already has one.
                    let mm_per_unit = design
                        .solve()
                        .ok()
                        .and_then(|solved| design.yield_report(&solved).mm_per_unit);
                    format!(
                        " -- volume {:.4} (model units^3){}",
                        m.volume,
                        external_proportions_note(&m, mm_per_unit)
                    )
                })
                .unwrap_or_default();
            (format!("Closed solid{volume_note}."), false)
        }
        Ok(SolidStatus::Degenerate {
            vertex_count,
            volume,
        }) => {
            // Same "(model units^3)" suffix as the `Closed`
            // arm above, so the unit is never ambiguous the way a bare
            // "volume 0.1234" with no unit would be.
            let volume_text = volume.map_or_else(
                || "non-finite".to_string(),
                |v| format!("{v:.4} (model units^3)"),
            );
            // `status()` only reaches `Degenerate` after a real `solve()` (it builds
            // the mesh from `Self::planes`, which requires one), so this re-solve
            // should always succeed -- `unwrap_or_default` degrades to no suspects
            // clause rather than panicking on that assumption if it's ever wrong.
            // `.ok()` shared below with the cheap plane-cap pre-filter, so the
            // common (not-over-cap) case pays for exactly this one solve, never two.
            let solved = design.solve().ok();
            if solved
                .as_deref()
                .is_some_and(solve_policy::likely_hit_plane_cap)
                && let Some(message) = solve_policy::too_many_planes_message(design)
            {
                return (message, true);
            }
            let suspects_note = solved.as_ref().map_or(String::new(), |solved| {
                degenerate_suspects_note(design, solved)
            });
            (
                format!(
                    "Degenerate: only {vertex_count} distinct vertex(es), volume \
                     {volume_text}{suspects_note}."
                ),
                true,
            )
        }
        Ok(SolidStatus::Unbounded { escaping }) => {
            // Same re-solve reasoning as the `Degenerate` arm above.
            let solved = design.solve().ok();
            if solved
                .as_deref()
                .is_some_and(solve_policy::likely_hit_plane_cap)
                && let Some(message) = solve_policy::too_many_planes_message(design)
            {
                return (message, true);
            }
            let tier_text = solved.as_ref().map_or_else(
                || format!("plane(s) {escaping:?}"),
                |solved| escaping_tier_text(design, solved, &escaping),
            );
            (
                format!("Unbounded: {tier_text} never close the solid."),
                true,
            )
        }
        // `MissingAnchor`'s own `Display` is already the full actionable remedy --
        // one complete sentence per named block (e.g. "Pavilion has no anchor: add a
        // tier with an exact scale value.") -- so it is shown verbatim rather than
        // wrapped in another sentence around it.
        Err(missing) => (missing.to_string(), true),
    }
}

/// [`status_text_and_is_problem`]'s counterpart for a caller that already has an
/// up-to-date `solved` mast list on hand.
///
/// Builds the mesh from [`Design::planes_from_solved`] instead of re-solving via
/// [`Design::status`]/[`Design::measure`].
///
/// There is no
/// `MissingAnchor` arm here: a caller holding a real `solved` slice already
/// knows the design solves.
#[must_use]
pub fn status_text_and_is_problem_from_solved(
    design: &Design,
    solved: &[SolvedTier],
) -> (String, bool) {
    // See `status_text_and_is_problem`'s identical guard just above.
    if design.tiers.is_empty() {
        return ("Preform only -- add tiers.".to_string(), false);
    }
    let planes = design.planes_from_solved(solved);
    match build_solid_mesh(&planes) {
        SolidStatus::Closed(_) => {
            // Same "independent re-mesh, not cached" reasoning
            // `status_text_and_is_problem`'s own `Closed` arm documents. Unlike
            // that arm, `solved` is already on hand here, so no extra `solve()`
            // is needed to read `yield_report`'s scale factor.
            let mm_per_unit = design.yield_report(solved).mm_per_unit;
            let volume_note = measure_solid(&planes)
                .map(|m| {
                    format!(
                        " -- volume {:.4} (model units^3){}",
                        m.volume,
                        external_proportions_note(&m, mm_per_unit)
                    )
                })
                .unwrap_or_default();
            (format!("Closed solid{volume_note}."), false)
        }
        SolidStatus::Degenerate {
            vertex_count,
            volume,
        } => {
            // `solved` may itself be
            // `solve_policy::solve_cancellably`'s own over-`MAX_PLANES` fallback (an
            // all-`SolveStrategy::Failed` list built with no real solve at all, see
            // that function's own doc comment) -- this caller's own doc comment
            // ("a caller holding a real `solved` slice already knows the design
            // solves") does not hold for that one case. The cheap pre-filter
            // (`solve_policy::likely_hit_plane_cap`) reads `solved`'s own tells with no
            // new solve at all; only a hit re-verifies via `too_many_planes_message`
            // (a real `solve_with`) for the authoritative numbers.
            if solve_policy::likely_hit_plane_cap(solved)
                && let Some(message) = solve_policy::too_many_planes_message(design)
            {
                return (message, true);
            }
            // See `status_text_and_is_problem`'s matching arm.
            let volume_text = volume.map_or_else(
                || "non-finite".to_string(),
                |v| format!("{v:.4} (model units^3)"),
            );
            let suspects_note = degenerate_suspects_note(design, solved);
            (
                format!(
                    "Degenerate: only {vertex_count} distinct vertex(es), volume \
                     {volume_text}{suspects_note}."
                ),
                true,
            )
        }
        SolidStatus::Unbounded { escaping } => {
            if solve_policy::likely_hit_plane_cap(solved)
                && let Some(message) = solve_policy::too_many_planes_message(design)
            {
                return (message, true);
            }
            let tier_text = escaping_tier_text(design, solved, &escaping);
            (
                format!("Unbounded: {tier_text} never close the solid."),
                true,
            )
        }
    }
}

/// Converts `design`'s current plane arrangement into the `GpuFacetPlane`s the render
/// thread's viewport already knows how to draw.
///
/// See this group's `mod.rs` doc comment
/// ("Feeding the viewport") for the sign-flip convention this inverts.
///
/// `design.planes()` returns `Result` ([`indicatrix_cut_core::MissingAnchor`] when
/// some block has no scale-reference tier yet). On `Err` this returns an empty plane
/// set rather than fabricating geometry: the viewport going blank is truthful (no
/// design to draw) rather than showing stale or invented facets.
#[must_use]
pub fn design_to_gpu_planes(design: &Design) -> Vec<GpuFacetPlane> {
    design
        .planes()
        .unwrap_or_default()
        .into_iter()
        .map(|(normal, offset)| GpuFacetPlane::new(normal.as_vec3(), -offset as f32))
        .collect()
}

/// [`design_to_gpu_planes`] plus the design's concave tools and where each came from:
/// everything the viewport needs to draw the whole stone.
///
/// The planes are exactly [`design_to_gpu_planes`]'s. The tools are
/// [`Design::concave_tools_from_solved`] of the design's own solve, with the
/// `(concave tier, placement)` of each primitive alongside. A design with no concave
/// tiers returns empty tool vectors without solving a second time, so a planar caller
/// pays nothing and sees nothing new. When the design does not solve, or its concave
/// tiers do not resolve (an invalid tier, too many placements), the tools are empty
/// too, for the same reason [`design_to_gpu_planes`] goes blank on an unsolvable
/// design: drawing the flat stone alone is truthful, a half-resolved set of tools is
/// not.
#[must_use]
pub fn design_to_gpu_geometry(
    design: &Design,
) -> (Vec<GpuFacetPlane>, Vec<ToolPrimitive>, ToolPlacements) {
    let planes = design_to_gpu_planes(design);
    if design.concave_tiers.is_empty() {
        return (planes, Vec::new(), Vec::new());
    }
    let (tools, placements) = design
        .solve()
        .ok()
        .and_then(|solved| design.concave_tools_from_solved(&solved).ok())
        .unwrap_or_default();
    (planes, tools, placements)
}

/// The tier search/filter box's substring test.
///
/// Slint's `string` has no `contains`/index-of operation, so this runs in Rust and is
/// exposed to `editor_tier_table.slint` via the `pure` `EditorModel. tier_matches_filter`
/// callback (see that property's own doc comment in `ui/models/editor.slint` for the
/// handoff this still needs).
///
/// Case-insensitive; an
/// empty `filter` always matches (every call site also short-circuits on this in
/// Slint before ever calling here, but this stays correct standalone too).
#[must_use]
pub fn tier_matches_filter(haystack: &str, filter: &str) -> bool {
    let filter = filter.trim();
    if filter.is_empty() {
        return true;
    }
    haystack.to_lowercase().contains(&filter.to_lowercase())
}
