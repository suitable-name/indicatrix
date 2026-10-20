//! Detached comparisons, including authored fields and independently solved masts.
use indicatrix::geometry::meet_solver::SolveControl;
use indicatrix_cut_core::Design;
use indicatrix_editor::snapshot::{DesignSnapshot, compare_rows};

pub(super) fn compare(old: Design, current: &Design, stop: &dyn Fn() -> bool) -> String {
    let control = SolveControl::with_stop(stop);
    let old_solved = old.solve_with(&control).ok();
    let current_solved = if stop() {
        None
    } else {
        current.solve_with(&control).ok()
    };
    let unavailable = old_solved.is_none() || current_solved.is_none();
    let snapshot = DesignSnapshot {
        design: old,
        solved: old_solved,
        label: "Historical step".into(),
        from_retarget_apply: false,
    };
    let mut lines = compare_rows(&snapshot, current, current_solved.as_deref())
        .into_iter()
        .filter(|row| row.status_label != "Same")
        .map(|row| {
            format!(
                "{} {}: {} → {} · mast Δ {} ({})",
                row.badge, row.name, row.old_angle, row.new_angle, row.mast_delta, row.status_label
            )
        })
        .collect::<Vec<_>>();
    let old = &snapshot.design;
    for (changed, label) in [
        (old.material != current.material, "Material"),
        (
            old.meta != current.meta,
            "Schedule metadata (headers, notes, gear, symmetry or index)",
        ),
        (
            // "Changed at all": the offset is an authored value, so any different stored
            // value counts, and the same bit pattern (even a NaN) does not. Comparing bits
            // is deliberate, not an approximate comparison.
            old.preform != current.preform
                || old.preform_y_offset.to_bits() != current.preform_y_offset.to_bits(),
            "Rough / preform",
        ),
        (
            old.girdle_diameter_mm != current.girdle_diameter_mm,
            "Girdle diameter",
        ),
        (
            old.tier_targets != current.tier_targets,
            "Dimensional targets",
        ),
        (
            old.tier_relations != current.tier_relations,
            "Angle relations",
        ),
        (old.tier_notes != current.tier_notes, "Tier notes"),
        (
            old.cheater_offsets_deg != current.cheater_offsets_deg,
            "Cheater offsets",
        ),
        (
            old.render_effects != current.render_effects,
            "Render effects",
        ),
        (extras_differ(old, current), "Preserved file properties"),
    ] {
        if changed {
            lines.push(format!("{label} differ"));
        }
    }
    for (index, (a, b)) in old.tiers.iter().zip(&current.tiers).enumerate() {
        if a.name != b.name {
            lines.push(format!("Tier {} name: {} → {}", index + 1, a.name, b.name));
        }
        if a.constraint != b.constraint
            || a.imported_meet != b.imported_meet
            || a.detached != b.detached
        {
            lines.push(format!("Tier {} meet instructions differ", index + 1));
        }
        if a.original_notes != b.original_notes {
            lines.push(format!("Tier {} original notes differ", index + 1));
        }
    }
    if lines.is_empty() {
        lines.push("No authoring differences.".into());
    }
    if unavailable {
        lines.push("Mast comparison unavailable: at least one state could not be solved.".into());
    }
    lines.join("\n")
}

fn extras_differ(a: &Design, b: &Design) -> bool {
    match (a.file_extras.as_deref(), b.file_extras.as_deref()) {
        (Some(a), Some(b)) => {
            a.top != b.top
                || a.preform != b.preform
                || a.material != b.material
                || a.schedule != b.schedule
                || a.history != b.history
                || a.tiers != b.tiers
                || a.concave_tiers != b.concave_tiers
        }
        (Some(a), None) | (None, Some(a)) => !a.is_empty(),
        (None, None) => false,
    }
}
