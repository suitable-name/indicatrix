//! `StandardGemCuts::from_asc_schedule` / `reconstruct_validated_brep_from_asc`
//! tests, backed by real `.asc` files pulled verbatim from `facet_diagrams.sqlite`
//! so the sign/offset conventions are checked against real designs whose published
//! `lw_ratio` / `facets_count` are known independently.

use glam::Vec3;
use indicatrix::{
    FacetSpec,
    geometry::{brep::GemPolyhedron, cuts::StandardGemCuts},
};
use indicatrix_formats::asc;

use crate::fixtures::assert_euler_formula;

// ---------------------------------------------------------------------------
// `StandardGemCuts::from_asc_schedule` / `reconstruct_validated_brep_from_asc`.
//
// These fixtures are real `.asc` files pulled verbatim from `facet_diagrams.sqlite`
// (via `indicatrix_formats::asc::parse_asc`), not hand-authored test data, so that the
// tests exercise the actual empirically-determined sign/offset conventions against
// real designs whose published `lw_ratio` / `facets_count` (from `diagram_details`)
// are known independently. See `apps/diagram-loader/examples/asc_corpus_report.rs` for the
// same cross-check run across the full corpus.
// ---------------------------------------------------------------------------

/// `attached_files` id 4208 ("pc45149.asc") -- "PC 45.149 Round Trichecker-12" by Fred
/// W. Van Sant. Published: `lw_ratio` = 1.000, `facets_count` = "36+12" (48 total).
const ASC_ROUND_TRICHECKER_12: &str = "GemCad 5.0\n\
g 96 0.0\n\
y 6 y\n\
I 1.72\n\
H PC 45.149  Round Trichecker-12\n\
H by Fred W. Van Sant, X 51, Extra Designs 2000\n\
H Released into the public domain in memory of Charles L. Moon\n\
a -41.000000 0.64991234 92 n 1 84 76 68 60 52 44 36 28 20 12 4\n\
a -90.000000 1.07325092 92 n 2 84 76 68 60 52 44 36 28 20 12 4\n\
a 29.730000 0.65249790 4 n A 12 20 28 36 44 52 60 68 76 84 92\n\
a 25.000000 0.59508784 96 n B 16 32 48 64 80\n\
a 10.000000 0.48799664 96 n C 16 32 48 64 80\n\
F \"For small stones\"\n";

/// `attached_files` id 4210 ("pc46019.asc") -- "PC 46.019 For Fun" by Michiko Huyhn.
/// Published: `lw_ratio` = 1.631, `facets_count` = "48+8" (56 total). Exercises an unsigned
/// zero-angle culet-like tier ("U") with no explicit crown/pavilion marker.
const ASC_FOR_FUN: &str = "GemCad 5.0\n\
g 96 0.0\n\
y 1 y\n\
I 1.54\n\
H PC 46.019  For Fun\n\
H by Michiko Huyhn\n\
a -44.864054 0.53791082 84 n P1 12 G Cut to mast depth X.\n\
a -50.185680 0.48593919 71 n P2 25 G Cut to mast depth X.\n\
a -48.722313 0.50066786 67 n P3 29 G Cut to mast depth X.\n\
a -43.200000 0.55323049 34 62 n P4 G Cut to mast depth X.\n\
a -90.000000 0.78956831 36 12 84 n G1 60 n G1 G Set stone size.\n\
a -90.000000 0.58736554 69 n G2 27 G Meet P1, P2, G1\n\
a -69.917066 0.54241199 69 n P5 27 G Level girdle.\n\
a -63.805515 0.54369547 65 n P6 31 G Meet P2, P3, P5\n\
a -90.000000 0.61916050 65 n G3 31 G Level girdle.\n\
a -50.270584 0.59003581 60 n P7 36 G Level girdle.\n\
a 54.575729 0.70935195 12 n C1 84 G Set girdle width.\n\
a 43.584337 0.48735616 27 n C2 69 G Level girdle.\n\
a 43.883301 0.51120026 65 31 n C3 G Level girdle.\n\
a 40.781358 0.60187658 60 36 n C4 G Level girdle.\n\
a 42.551058 0.68288357 10 n C5 86 G Meet G1, C1\n\
a 42.551058 0.62897635 14 n C6 82 G Meet G1, G2, C1, C2\n\
a 36.064955 0.49509639 24 n C7 72 G Meet G1, G2, C1, C2, C6\n\
a 39.972596 0.47212999 28 n C8 68 G Meet G2, G3, C2, C3\n\
a 40.114031 0.47905350 29 n C9 67 G Meet G2, G3, C2, C3, C8\n\
a 40.468111 0.51438869 64 32 n C10 G Meet G1, G3, C3, C4\n\
a 37.224985 0.62845267 12 n C11 84 G Meet C1, C5, C6\n\
a 29.739521 0.46322909 26 n C12 70 G Meet C2, C7, C8; C6, C7, C11\n\
a 36.239767 0.49263893 31 n C13 65 G Meet C3, C9, C10\n\
a 22.049986 0.47954099 65 31 n C14 G Meet C8, C9, C12, C13; C10, C13\n\
a 6.000000 0.46712247 48 n C15 G Meet C10, C13, C14\n\
a 21.597048 0.45849715 26 n C16 70 G Meet C6, C7, C11, C12; C8, C9, C12, C13, C14\n\
a 24.656793 0.57799286 12 n C17 84 G Meet C5, C11; C6, C7, C11, C12, C16\n\
a 0.000000 0.44755829 96 n U\n\
F Also USFG Newsletter Sep 2013, Facets Jan 2014\n";

/// `attached_files` id 4430 ("pc42060.asc") -- "PC 42.060 Large Texas Star" by
/// Charles `McCoy`. Published: `lw_ratio` = 1.051, `facets_count` = "41+10" (51 total). Gear=80 (not
/// the far more common 96), symmetry order 5 -- exercises both away from the
/// dominant convention, plus an explicit table tier at unsigned zero.
const ASC_LARGE_TEXAS_STAR: &str = "GemCad 5.0\n\
g 80 0.0\n\
y 5 y\n\
I 1.61\n\
H PC 42.060  Large Texas Star\n\
H by Charles McCoy\n\
a -40.000000 0.54589773 76 n 1 68 60 52 44 36 28 20 12 4 G TCP\n\
a -90.000000 1.05672946 76 n 2 68 60 52 44 36 28 20 12 4 G Size stone\n\
a -67.800000 0.78700478 76 n 3 68 60 52 44 36 28 20 12 4 G Determine the size of the star\n\
a -37.310000 0.53454720 78 n 4 66 62 50 46 34 30 18 14 2 G MP 1-3\n\
a 40.000000 1.11585176 4 n A 12 20 28 36 44 52 60 68 76 G Establish girdle thickness\n\
a 0.000000 0.72641642 80 n T G Make table large enough to show all of the star\n\
F Leave #4 frosted\n";

/// `attached_files` id 4422 ("pc43001a.asc") -- "PC 43.001A Shah (Replica)". No facet
/// names anywhere, a rare negative-mast tier at an unsigned zero angle, and two
/// tiers (`-90 ... 0 32` and a later `-90 ... 0`) that share an index and mast,
/// producing a literal duplicate half-space plane. Exercises `dedup_planes` and the
/// "no name at all" parsing path, not the L/W or facet-count cross-check (this
/// design's own footnote flags it as disagreeing with the reference it's replicating).
const ASC_SHAH_REPLICA_NO_NAMES: &str = "GemCad 4.51\n\
g 64 64.0\n\
y 1 n\n\
I 1.54\n\
H PC 43.001A Shah (Replica)\n\
a -90.00 1.00000 16\n\
a -90.00 0.44700 0 32\n\
a 0.00 -0.36800 0\n\
a -90.00 0.99518 49 47\n\
a 1.87 0.34210 49 47\n\
a -90.00 0.44700 0\n\
a 69.84 0.87020 16\n\
a 85.00 0.44530 0\n\
a -71.11 0.90450 48\n\
a 20.59 0.39882 30\n\
a 24.47 0.38860 1.7\n\
F Does not agree with Barbour's 43.001. Glass replica has rounded facets on the ends.\n";

/// Same length/width measure the corpus-wide report in
/// `apps/diagram-loader/examples/asc_corpus_report.rs` uses: the longest chord across the reconstructed
/// girdle outline as length, and the outline's extent perpendicular to that chord as
/// width.
fn length_width_ratio(hull: &GemPolyhedron) -> f64 {
    let outline = hull.girdle_outline();
    let mut best = (0usize, 0usize, 0.0f32);
    for i in 0..outline.len() {
        for j in (i + 1)..outline.len() {
            let d = (outline[i] - outline[j]).length();
            if d > best.2 {
                best = (i, j, d);
            }
        }
    }
    let length = best.2;
    let dir = (outline[best.1] - outline[best.0]).normalize();
    let perp = Vec3::new(-dir.z, 0.0, dir.x);
    let (mut min_p, mut max_p) = (f32::MAX, f32::MIN);
    for p in &outline {
        let proj = p.dot(perp);
        min_p = min_p.min(proj);
        max_p = max_p.max(proj);
    }
    f64::from(length / (max_p - min_p))
}

#[test]
fn asc_real_designs_reconstruct_closed_solids_with_no_untouched_planes() {
    for (label, content) in [
        ("Round Trichecker-12", ASC_ROUND_TRICHECKER_12),
        ("For Fun", ASC_FOR_FUN),
        ("Large Texas Star", ASC_LARGE_TEXAS_STAR),
        ("Shah Replica (no names)", ASC_SHAH_REPLICA_NO_NAMES),
    ] {
        let schedule = asc::parse_asc(content)
            .unwrap_or_else(|e| panic!("{label}: real .asc sample must parse: {e}"));
        let hull =
            StandardGemCuts::reconstruct_validated_brep_from_asc(&schedule).unwrap_or_else(|e| {
                panic!("{label}: real .asc sample must reconstruct a valid closed solid: {e}")
            });
        assert_euler_formula(&hull);
        assert!(
            hull.untouched_planes().is_empty(),
            "{label}: every plane in a real, well-formed schedule should be touched"
        );
        assert!(
            hull.volume().is_finite() && hull.volume() > 0.0,
            "{label}: volume must be finite and positive"
        );
    }
}

#[test]
fn asc_round_trichecker_12_matches_published_lw_ratio_and_facet_count() {
    let schedule = asc::parse_asc(ASC_ROUND_TRICHECKER_12).unwrap();
    let hull = StandardGemCuts::reconstruct_validated_brep_from_asc(&schedule).unwrap();
    assert_eq!(
        hull.facet_planes.len(),
        48,
        "published facets_count is \"36+12\" = 48"
    );
    let lw = length_width_ratio(&hull);
    assert!(
        (lw - 1.000).abs() < 0.02,
        "published lw_ratio is 1.000, got {lw:.4}"
    );
}

#[test]
fn asc_for_fun_matches_published_lw_ratio_and_facet_count() {
    // Tier-count/angle/distance assertions on top of indicatrix_formats::asc's own
    // parser-level unit tests: 28 tiers (10 pavilion/girdle + 17 crown + 1 culet), the
    // P1 tier's angle and mast distance, and the geometry this schedule produces.
    let schedule = asc::parse_asc(ASC_FOR_FUN).unwrap();
    assert_eq!(schedule.tiers.len(), 28);
    assert_eq!(schedule.gear_teeth, 96);
    assert!((schedule.refractive_index - 1.54).abs() < 1e-9);
    assert!((schedule.tiers[0].angle_deg - (-44.864_054)).abs() < 1e-6);
    assert!((schedule.tiers[0].mast - 0.537_910_82).abs() < 1e-9);

    let hull = StandardGemCuts::reconstruct_validated_brep_from_asc(&schedule).unwrap();
    assert_eq!(
        hull.facet_planes.len(),
        56,
        "published facets_count is \"48+8\" = 56"
    );
    let lw = length_width_ratio(&hull);
    assert!(
        (lw - 1.631).abs() < 0.02,
        "published lw_ratio is 1.631, got {lw:.4}"
    );
}

#[test]
fn asc_large_texas_star_matches_published_lw_ratio_and_facet_count() {
    // gear=80 (not 96) and an explicit unsigned-zero table tier ("T") -- both away
    // from the more common case the other fixtures exercise.
    let schedule = asc::parse_asc(ASC_LARGE_TEXAS_STAR).unwrap();
    assert_eq!(schedule.gear_teeth, 80);
    assert_eq!(schedule.symmetry_order, 5);

    let hull = StandardGemCuts::reconstruct_validated_brep_from_asc(&schedule).unwrap();
    assert_eq!(
        hull.facet_planes.len(),
        51,
        "published facets_count is \"41+10\" = 51"
    );
    let lw = length_width_ratio(&hull);
    assert!(
        (lw - 1.051).abs() < 0.02,
        "published lw_ratio is 1.051, got {lw:.4}"
    );
}

#[test]
fn asc_schedules_always_satisfy_from_planes_d_negative_precondition() {
    for content in [
        ASC_ROUND_TRICHECKER_12,
        ASC_FOR_FUN,
        ASC_LARGE_TEXAS_STAR,
        ASC_SHAH_REPLICA_NO_NAMES,
    ] {
        let schedule = asc::parse_asc(content).unwrap();
        for p in StandardGemCuts::from_asc_schedule(&schedule) {
            assert!(
                p.d < 0.0,
                "from_asc_schedule produced a plane with d = {} >= 0",
                p.d
            );
        }
    }
}

#[test]
fn asc_schedule_negative_mast_and_duplicate_tiers_do_not_panic_or_break_geometry() {
    // ASC_SHAH_REPLICA_NO_NAMES has a negative-mast tier ("0.00 -0.36800 0") and two
    // tiers that produce a literal duplicate half-space plane (index 0 at angle -90,
    // mast 0.44700, listed on two separate rows). Neither should panic, and the
    // duplicate should be silently absorbed by dedup_planes rather than tripping
    // GemPolyhedron::from_planes's "coincident planes" rejection.
    let schedule = asc::parse_asc(ASC_SHAH_REPLICA_NO_NAMES).unwrap();
    let planes = StandardGemCuts::from_asc_schedule(&schedule);
    for p in &planes {
        assert!(
            p.d < 0.0,
            "negative mast must still produce d < 0 (magnitude, not sign, is used), got d={}",
            p.d
        );
    }
    let hull = GemPolyhedron::from_planes(planes)
        .expect("duplicate half-space planes must be deduped before reaching from_planes");
    assert_euler_formula(&hull);
}

#[test]
fn asc_geometry_falls_back_cleanly_when_no_asc_is_available() {
    // Designs without an attached .asc (about 4.8% of the catalog) must still render
    // via the existing angle_settings-based path -- from_asc_schedule/
    // reconstruct_validated_brep_from_asc are purely additive.
    let angles = vec![FacetSpec {
        facet: "C1".into(),
        angle: "40.00\u{b0}".into(),
        index: "8 girdle facets".into(),
        notes: String::new(),
    }];
    let planes = StandardGemCuts::from_database_angles(&angles, 96);
    assert!(
        !planes.is_empty(),
        "the angle_settings fallback path must remain usable independent of from_asc_schedule"
    );
}
