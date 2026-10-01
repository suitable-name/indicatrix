//! Console-report formatting for one strategy's aggregate results: per-`SolveStrategy`
//! breakdown, exact/fallback rates by `ConstraintKind` and named-resolution bucket,
//! relative-error percentiles, and small median/percentage helpers shared with
//! `verified_extras`.

use indicatrix::geometry::meet_solver::SolveStrategy;

use crate::types::{ConstraintKind, TierResult};

/// Prints every aggregate table of the validation report.
#[expect(
    clippy::too_many_lines,
    reason = "straight-line report printing in a temporary probe; splitting it up would \
              scatter the report's structure across helper functions for no clarity gain"
)]
pub fn print_report(
    parse_ok: usize,
    parse_err: usize,
    no_scale_reference_at_all: usize,
    tier_results: &[TierResult],
    design_worst_err: &[Option<f64>],
    (designs_with_meet_named, designs_with_meet_existing, designs_with_scale_reference): (
        usize,
        usize,
        usize,
    ),
) {
    println!("\n=== Corpus coverage ===");
    println!("  designs parsed OK:  {parse_ok}");
    println!("  designs parse error: {parse_err}");
    println!(
        "  designs needing >=1 block's real-mast fallback (see this report's header): {no_scale_reference_at_all}"
    );
    println!("  designs with >=1 MeetNamed tier:      {designs_with_meet_named}");
    println!("  designs with >=1 MeetExisting tier:   {designs_with_meet_existing}");
    println!("  designs with >=1 ScaleReference tier: {designs_with_scale_reference}");

    println!(
        "\n=== Strategy usage ({} meet-derived tiers, scale-reference tiers excluded) ===",
        tier_results.len()
    );
    for strategy in [
        SolveStrategy::DependencyOrder,
        SolveStrategy::JointGroup,
        SolveStrategy::LeastSquaresFallback,
        SolveStrategy::Failed,
    ] {
        let n = tier_results
            .iter()
            .filter(|t| t.strategy == strategy)
            .count();
        println!("  {strategy:?}: {n} ({:.1}%)", pct(n, tier_results.len()));
    }
    let exact = tier_results
        .iter()
        .filter(|t| {
            matches!(
                t.strategy,
                SolveStrategy::DependencyOrder | SolveStrategy::JointGroup
            )
        })
        .count();
    let fallback = tier_results
        .iter()
        .filter(|t| {
            matches!(
                t.strategy,
                SolveStrategy::LeastSquaresFallback | SolveStrategy::Failed
            )
        })
        .count();
    println!(
        "  -- exact (DependencyOrder+JointGroup): {exact} ({:.1}%)",
        pct(exact, tier_results.len())
    );
    println!(
        "  -- fallback (LeastSquares+Failed):     {fallback} ({:.1}%)",
        pct(fallback, tier_results.len())
    );

    println!("\n=== Overall (all meet-derived tiers, blended) ===");
    let mut all_errs: Vec<f64> = tier_results.iter().map(|t| t.rel_err).collect();
    all_errs.sort_by(|a, b| a.partial_cmp(b).unwrap());
    println!(
        "  exact-match rate (DependencyOrder+JointGroup): {:.1}%",
        pct(exact, tier_results.len())
    );
    println!("  median relative error: {:.4}", median(&all_errs));

    println!("\n=== Split by ConstraintKind (do not blend) ===");
    for kind in [
        ConstraintKind::ScaleReference,
        ConstraintKind::MeetNamed,
        ConstraintKind::MeetExisting,
    ] {
        let subset: Vec<&TierResult> = tier_results.iter().filter(|t| t.kind == kind).collect();
        if subset.is_empty() {
            println!("  {kind:?}: 0 tiers (excluded from scoring, or none present)");
            continue;
        }
        let exact = subset
            .iter()
            .filter(|t| {
                matches!(
                    t.strategy,
                    SolveStrategy::DependencyOrder | SolveStrategy::JointGroup
                )
            })
            .count();
        let joint = subset
            .iter()
            .filter(|t| t.strategy == SolveStrategy::JointGroup)
            .count();
        println!(
            "  {kind:?}: {} tiers, {:.1}% exact ({:.1}% via JointGroup specifically), median rel. err {:.4}",
            subset.len(),
            pct(exact, subset.len()),
            pct(joint, subset.len()),
            median(&subset.iter().map(|t| t.rel_err).collect::<Vec<_>>())
        );
    }

    println!("\n=== MeetNamed split by name resolution (the real gate is on -resolved) ===");
    println!(
        "  (resolution measured against the solver's own name_to_tier/girdle_tier logic \
         -- see classify_named_resolution -- not a post-hoc filter)"
    );
    for (label, want_resolved) in [
        ("MeetNamed-resolved", true),
        ("MeetNamed-unresolved", false),
    ] {
        let subset: Vec<&TierResult> = tier_results
            .iter()
            .filter(|t| {
                t.kind == ConstraintKind::MeetNamed && t.named_resolved == Some(want_resolved)
            })
            .collect();
        if subset.is_empty() {
            println!("  {label}: 0 tiers");
            continue;
        }
        let exact = subset
            .iter()
            .filter(|t| {
                matches!(
                    t.strategy,
                    SolveStrategy::DependencyOrder | SolveStrategy::JointGroup
                )
            })
            .count();
        let used: Vec<&&TierResult> = subset.iter().filter(|t| t.used_named).collect();
        let unused: Vec<&&TierResult> = subset.iter().filter(|t| !t.used_named).collect();
        println!(
            "  {label}: {} tiers, {:.1}% exact (DependencyOrder+JointGroup), median rel. err {:.4}",
            subset.len(),
            pct(exact, subset.len()),
            median(&subset.iter().map(|t| t.rel_err).collect::<Vec<_>>())
        );
        println!(
            "    of which constructively used named refs: {} (median rel. err {:.4}); \
             fell back to rank-1: {} (median rel. err {:.4})",
            used.len(),
            median(&used.iter().map(|t| t.rel_err).collect::<Vec<_>>()),
            unused.len(),
            median(&unused.iter().map(|t| t.rel_err).collect::<Vec<_>>())
        );
        for (cause, tag) in [
            ("refs not settled at release", 'u'),
            ("no incident feasible level", 'n'),
        ] {
            let sub: Vec<f64> = unused
                .iter()
                .filter(|t| t.fallback_cause == tag)
                .map(|t| t.rel_err)
                .collect();
            println!(
                "      fallback cause `{cause}`: {} tiers (median rel. err {:.4})",
                sub.len(),
                median(&sub)
            );
        }
    }

    println!("\n=== Per-tier relative error distribution (meet-derived tiers only) ===");
    let mut errs: Vec<f64> = tier_results.iter().map(|t| t.rel_err).collect();
    errs.sort_by(|a, b| a.partial_cmp(b).unwrap());
    print_percentiles(&errs);

    println!(
        "\n=== Per-tier relative error, exact strategies only (DependencyOrder+JointGroup) ==="
    );
    let mut exact_errs: Vec<f64> = tier_results
        .iter()
        .filter(|t| {
            matches!(
                t.strategy,
                SolveStrategy::DependencyOrder | SolveStrategy::JointGroup
            )
        })
        .map(|t| t.rel_err)
        .collect();
    exact_errs.sort_by(|a, b| a.partial_cmp(b).unwrap());
    print_percentiles(&exact_errs);

    println!("\n=== Per-design success (worst meet-derived tier's relative error) ===");
    let scored: Vec<f64> = design_worst_err.iter().filter_map(|w| *w).collect();
    let no_meet_tiers = design_worst_err.iter().filter(|w| w.is_none()).count();
    println!("  designs with >=1 meet-derived tier: {}", scored.len());
    println!("  designs with zero meet-derived tiers (all scale-reference): {no_meet_tiers}");
    for threshold in [0.01, 0.10] {
        let within = scored.iter().filter(|&&e| e <= threshold).count();
        println!(
            "  every meet-derived tier within {:.0}%: {within} ({:.1}%)",
            threshold * 100.0,
            pct(within, scored.len())
        );
    }
    let mut sorted_scored = scored;
    sorted_scored.sort_by(|a, b| a.partial_cmp(b).unwrap());
    println!("  worst-tier-per-design distribution:");
    print_percentiles(&sorted_scored);
}

fn print_percentiles(sorted: &[f64]) {
    if sorted.is_empty() {
        println!("  (no data)");
        return;
    }
    let n = sorted.len();
    let at = |p: f64| sorted[((n as f64 * p) as usize).min(n - 1)];
    println!(
        "  p10={:.4} p25={:.4} median={:.4} p75={:.4} p90={:.4} p99={:.4} max={:.4}",
        at(0.10),
        at(0.25),
        at(0.50),
        at(0.75),
        at(0.90),
        at(0.99),
        sorted[n - 1]
    );
}

/// Upper median of the values, zero when empty.
pub fn median(vals: &[f64]) -> f64 {
    let mut v = vals.to_vec();
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    if v.is_empty() { 0.0 } else { v[v.len() / 2] }
}

/// Percentage of `n` in `total`, zero when `total` is zero.
pub fn pct(n: usize, total: usize) -> f64 {
    if total == 0 {
        0.0
    } else {
        100.0 * n as f64 / total as f64
    }
}
