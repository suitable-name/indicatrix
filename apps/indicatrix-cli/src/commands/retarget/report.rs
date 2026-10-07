//! The `retarget` report: the text and the JSON a run prints.

use super::{Decision, SearchFacts};
use crate::{
    args::{Mode, RetargetArgs, lighting_name, preset_name},
    commands::{key_values, list_block},
    format::{
        ANGLE_DECIMALS, Align, INDEX_DECIMALS, PERCENT_DECIMALS, count_noun, fixed, json_number,
        json_number32, json_text, table,
    },
    load::Loaded,
    materials::{Scoring, label_of},
};
use indicatrix_cut_core::{Design, critical_angle_deg};
use indicatrix_editor::retarget::{
    CrownShift, RetargetPlan,
    metrics::{MetricColumn, RetargetMetrics},
    plan::tier_display_names,
    validity::ValidityStatus,
};
use serde_json::{Value, json};

/// What the report is about.
pub(super) struct Report<'a> {
    pub(super) loaded: &'a Loaded,
    pub(super) args: &'a RetargetArgs,
    pub(super) plan: &'a RetargetPlan,
    pub(super) target: &'a Scoring,
    pub(super) decision: &'a Decision,
    pub(super) after: Option<&'a Design>,
    pub(super) written: Option<String>,
    pub(super) notes: Vec<String>,
}

/// The word for a verdict.
const fn status_word(status: ValidityStatus) -> &'static str {
    match status {
        ValidityStatus::Valid => "valid",
        ValidityStatus::Invalid => "invalid",
        ValidityStatus::Unchecked => "unchecked",
    }
}

/// How the design's own material reads in the report.
fn from_label(design: &Design, n_from: f64) -> String {
    let material = &design.material;
    if material.name.is_none() && material.refractive_index_override.is_none() {
        format!(
            "no material named, refractive index {}",
            fixed(n_from, INDEX_DECIMALS)
        )
    } else {
        label_of(material, n_from)
    }
}

/// How the crown policy reads in the report.
fn crown_text(crown: CrownShift) -> String {
    if crown.scale_by_ratio {
        "scaled by the ratio of the critical angles".to_string()
    } else if crown.follow_pavilion {
        "follows the pavilion's stretch".to_string()
    } else if crown.fraction == 0.0 {
        "left alone".to_string()
    } else {
        format!(
            "{} of the pavilion's critical-angle shift",
            fixed(crown.fraction, PERCENT_DECIMALS)
        )
    }
}

/// How the mode reads in the report.
fn mode_text(args: &RetargetArgs, search: Option<&SearchFacts>) -> String {
    match (args.mode, search) {
        (Mode::Optimize, Some(facts)) => format!(
            "optimize ({}, within {} degrees, budget {}, seed {}{}; {} evaluations spent on {} free \
             {}, {} valid {})",
            preset_name(facts.preset),
            fixed(facts.range_deg, 1),
            facts.budget,
            facts.seed,
            if facts.keep_look {
                ", keeping the look"
            } else {
                ""
            },
            facts.evaluations,
            facts.free_tiers,
            if facts.free_tiers == 1 {
                "angle"
            } else {
                "angles"
            },
            facts.options,
            if facts.options == 1 {
                "option"
            } else {
                "options"
            },
        ),
        (Mode::Optimize, None) => "optimize".to_string(),
        (Mode::Shift, _) => "shift".to_string(),
    }
}

/// The tiers whose angle differs between `before` and `after`, as table rows.
///
/// The report is read by a person: the tier is named the way the tier table names it (an
/// old-style `3` reads as its standard code), both angles are magnitudes, and the change is the
/// difference of those two numbers with an explicit sign (`+0.5` is half a degree steeper
/// whichever side of the girdle the tier is on). The JSON report keeps the stored signed angles.
fn change_rows(before: &Design, after: &Design) -> Vec<Vec<String>> {
    let names = tier_display_names(before);
    before
        .tiers
        .iter()
        .zip(&after.tiers)
        .enumerate()
        .filter(|(_, (was, now))| was.angle_deg.to_bits() != now.angle_deg.to_bits())
        .map(|(index, (was, now))| {
            vec![
                (index + 1).to_string(),
                names.get(index).cloned().unwrap_or_default(),
                fixed(was.angle_deg.abs(), ANGLE_DECIMALS),
                fixed(now.angle_deg.abs(), ANGLE_DECIMALS),
                format!(
                    "{:+.*}",
                    ANGLE_DECIMALS,
                    now.angle_deg.abs() - was.angle_deg.abs()
                ),
            ]
        })
        .collect()
}

/// The three optical columns as a table.
fn metrics_table(metrics: &RetargetMetrics) -> String {
    let cells = metrics.cells();
    let labels = ["Windowing", "Brilliance", "Extinction"];
    let rows: Vec<Vec<String>> = labels
        .iter()
        .zip(cells.chunks(3))
        .map(|(label, row)| {
            let mut line = vec![(*label).to_string()];
            line.extend(row.iter().cloned());
            line
        })
        .collect();
    table(
        &[
            "",
            "Current in current",
            "Current in target",
            "Retargeted in target",
        ],
        &[Align::Left, Align::Right, Align::Right, Align::Right],
        &rows,
    )
}

/// The header lines of the text report.
fn header_rows(report: &Report<'_>) -> Vec<(&'static str, String)> {
    let (plan, decision) = (report.plan, report.decision);
    let mut rows = vec![
        (
            "Design",
            format!("{} ({})", report.loaded.name, report.loaded.file_name),
        ),
        (
            "Material",
            format!(
                "{} -> {}",
                from_label(&report.loaded.design, plan.n_from),
                report.target.label
            ),
        ),
        (
            "Index",
            format!(
                "{} -> {}",
                fixed(plan.n_from, INDEX_DECIMALS),
                fixed(plan.n_to, INDEX_DECIMALS)
            ),
        ),
        (
            "Critical",
            format!(
                "{} -> {} degrees",
                fixed(critical_angle_deg(plan.n_from), PERCENT_DECIMALS),
                fixed(critical_angle_deg(plan.n_to), PERCENT_DECIMALS)
            ),
        ),
        ("Mode", mode_text(report.args, decision.search.as_ref())),
        ("Crown", crown_text(plan.crown)),
        ("Verdict", decision.validity.headline()),
    ];
    if decision.anchors > 0 {
        rows.push((
            "Masts",
            format!(
                "{} turned about their girdle edges",
                count_noun(decision.anchors, "tier", "tiers")
            ),
        ));
    }
    rows.push((
        "Result",
        match (&decision.change, &report.written) {
            (Err(_), _) => "refused, nothing was written".to_string(),
            (Ok(_), Some(path)) => format!("applied, saved to {path}"),
            (Ok(_), None) => "applied (not saved: give --out FILE to save it)".to_string(),
        },
    ));
    rows
}

/// The text report.
pub(super) fn text_report(report: &Report<'_>) -> String {
    let decision = report.decision;
    let mut text = key_values(&header_rows(report));
    let mut blocks = vec![list_block("Details", &decision.validity.detail_lines())];
    if let Some(after) = report.after {
        let changed = change_rows(&report.loaded.design, after);
        blocks.push(format!(
            "Changed tiers ({}):\n{}",
            changed.len(),
            table(
                &["Row", "Tier", "From", "To", "Change"],
                &[
                    Align::Right,
                    Align::Left,
                    Align::Right,
                    Align::Right,
                    Align::Right
                ],
                &changed
            )
        ));
        if decision.metrics.any() {
            blocks.push(format!(
                "Optical figures (table up, light {}):\n{}",
                lighting_name(report.args.lighting),
                metrics_table(&decision.metrics)
            ));
        }
    }
    blocks.push(list_block("Notes", &report.notes));
    for block in blocks.iter().filter(|block| !block.is_empty()) {
        text.push('\n');
        text.push_str(block);
    }
    text
}

/// One metrics column as JSON.
fn column_json(column: Option<MetricColumn>) -> Value {
    column.map_or(Value::Null, |column| {
        json!({
            "windowing_pct": json_number32(column.windowing_pct, 2),
            "brilliance_pct": json_number32(column.brilliance_pct, 2),
            "extinction_pct": json_number32(column.extinction_pct, 2),
        })
    })
}

/// One end of the retarget as JSON.
fn end_json(label: &str, n_d: f64) -> Value {
    json!({
        "material": label,
        "refractive_index": json_number(n_d, 4),
        "critical_angle_deg": json_number(critical_angle_deg(n_d), 2),
    })
}

/// The tiers that moved, as JSON objects.
fn changes_json(before: &Design, after: Option<&Design>) -> Vec<Value> {
    let Some(after) = after else {
        return Vec::new();
    };
    before
        .tiers
        .iter()
        .zip(&after.tiers)
        .enumerate()
        .filter(|(_, (was, now))| was.angle_deg.to_bits() != now.angle_deg.to_bits())
        .map(|(index, (was, now))| {
            json!({
                "row": index + 1,
                "tier": was.name,
                "from_deg": json_number(was.angle_deg, 4),
                "to_deg": json_number(now.angle_deg, 4),
            })
        })
        .collect()
}

/// What the search did, as JSON; null for a Shift retarget.
fn search_json(search: Option<&SearchFacts>) -> Value {
    search.map_or(Value::Null, |facts| {
        json!({
            "objective": preset_name(facts.preset),
            "range_deg": json_number(facts.range_deg, 2),
            "budget": facts.budget,
            "seed": facts.seed,
            "keep_look": facts.keep_look,
            "evaluations": facts.evaluations,
            "free_tiers": facts.free_tiers,
            "options": facts.options,
        })
    })
}

/// The JSON report.
pub(super) fn json_report(report: &Report<'_>) -> String {
    let (plan, decision) = (report.plan, report.decision);
    let changes = changes_json(&report.loaded.design, report.after);
    let search = search_json(decision.search.as_ref());
    let mode = match report.args.mode {
        Mode::Shift => "shift",
        Mode::Optimize => "optimize",
    };
    json_text(&json!({
        "design": report.loaded.name,
        "file": report.loaded.file_name,
        "from": end_json(&from_label(&report.loaded.design, plan.n_from), plan.n_from),
        "to": end_json(&report.target.label, plan.n_to),
        "mode": mode,
        "crown": {
            "fraction": json_number(plan.crown.fraction, 4),
            "scale_by_ratio": plan.crown.scale_by_ratio,
            "follow_pavilion": plan.crown.follow_pavilion,
        },
        "lighting": lighting_name(report.args.lighting),
        "applied": report.after.is_some(),
        "verdict": {
            "status": status_word(decision.validity.status),
            "headline": decision.validity.headline(),
            "details": decision.validity.detail_lines(),
        },
        "changes": changes,
        "re_anchored": decision.anchors,
        "metrics": {
            "current_in_current": column_json(decision.metrics.current_in_current),
            "current_in_target": column_json(decision.metrics.current_in_target),
            "retargeted_in_target": column_json(decision.metrics.retargeted_in_target),
        },
        "search": search,
        "written": report.written,
        "notes": report.notes,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing;

    #[test]
    fn the_change_table_reads_magnitudes_codes_and_a_signed_difference() {
        let mut before = testing::brilliant_at_172();
        let tier = before
            .tier_position_by_name("Pavilion Main")
            .expect("the standard brilliant has a Pavilion Main");
        before.tiers[tier].name = "3".to_string();
        before.tiers[tier].angle_deg = -41.0;
        let mut after = before.clone();
        after.tiers[tier].angle_deg = -40.0;

        let code = indicatrix_cut_core::compute_tier_labels(&before.tiers)[tier]
            .code
            .clone();
        let rows = change_rows(&before, &after);
        // The stored angles are negative; the report shows 41 and 40 and a signed difference.
        assert_eq!(
            rows,
            vec![vec![
                (tier + 1).to_string(),
                code,
                "41.0000".to_string(),
                "40.0000".to_string(),
                "-1.0000".to_string(),
            ]]
        );
        assert_ne!(rows[0][1], "3", "the old-style name is not shown");

        // JSON keeps the stored signed angles.
        let changes = changes_json(&before, Some(&after));
        assert_eq!(changes[0]["from_deg"], -41.0);
        assert_eq!(changes[0]["to_deg"], -40.0);
    }
}
