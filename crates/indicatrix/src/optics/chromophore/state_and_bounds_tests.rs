//! The valence state of a host is the way it is in reality, and the amount bounds follow it.
//!
//! - The second species of glass copper (the `Cu0` colloid of copper ruby) and of quartz iron (the
//!   interstitial `Fe2+` of prasiolite) exists only after the treatment that makes it
//!   (`reducing_strike`, `thermal_anneal_550`), so a single-element recipe never raises the
//!   advisory `Relation` warning against itself.
//! - An element's bound is taken under the recipe's treatments (`element_conc_max_split`): a
//!   species whose fraction is 0 cannot absorb and contributes none.
//! - The elements of a multi-element chromophore are bounded together (`cap_summed_amounts`).

use super::*;
use crate::optics::absorption::AbsorptionBand;

fn cat() -> &'static ChromophoreCatalogue {
    ChromophoreCatalogue::global()
}

fn peak_near(bands: &[AbsorptionBand], nm: f32) -> Option<f32> {
    bands
        .iter()
        .find(|b| (b.center_nm - nm).abs() < 1.0)
        .map(|b| b.peak)
}

fn recipe(host: &str, entries: &[(&str, f64)], treatments: &[&str]) -> ColorRecipe {
    let mut r = ColorRecipe::new(host, cat().data_version);
    for (id, amount) in entries {
        assert!(r.set_amount(id, *amount), "{id} amount {amount} rejected");
    }
    r.treatments = treatments.iter().map(|t| (*t).to_string()).collect();
    r
}

fn has_relation(warnings: &[ResolveWarning]) -> bool {
    warnings
        .iter()
        .any(|w| matches!(w, ResolveWarning::Relation { .. }))
}

fn has_clamp(warnings: &[ResolveWarning]) -> bool {
    warnings
        .iter()
        .any(|w| matches!(w, ResolveWarning::AmountClamped { .. }))
}

/// `(requested, used)` of the `AmountClamped` warning of `id`.
fn clamped(warnings: &[ResolveWarning], id: &str) -> Option<(f64, f64)> {
    warnings.iter().find_map(|w| match w {
        ResolveWarning::AmountClamped {
            id: got,
            requested,
            used,
        } if got == id => Some((*requested, *used)),
        _ => None,
    })
}

#[test]
fn glass_copper_is_blue_until_the_reducing_strike() {
    let host = cat().host("glass_silicate").expect("glass host");

    // Untreated: Cu2+ (790 nm), no colloid (570 nm), no relation warning.
    let mut r = recipe("glass_silicate", &[("Cu", 1.0)], &[]);
    let (t, w) = resolve(&r, cat()).expect("resolves");
    assert!(
        peak_near(&t.o_ray, 790.0).is_some(),
        "Cu2+ band: {:?}",
        t.o_ray
    );
    assert!(
        peak_near(&t.o_ray, 570.0).is_none(),
        "colloid: {:?}",
        t.o_ray
    );
    assert!(!has_relation(&w), "{w:?}");

    // The strike moves the copper to the colloid.
    r.treatments.push("reducing_strike".into());
    let (t, w) = resolve(&r, cat()).expect("resolves");
    assert!(
        peak_near(&t.o_ray, 570.0).is_some(),
        "colloid: {:?}",
        t.o_ray
    );
    assert!(peak_near(&t.o_ray, 790.0).is_none(), "Cu2+: {:?}", t.o_ray);
    assert!(!has_relation(&w), "{w:?}");
    assert!(
        !w.iter()
            .any(|x| matches!(x, ResolveWarning::TreatmentSkipped { .. })),
        "{w:?}"
    );

    // Bounds: 8 wt% CuO untreated, the colloid's 10000 ppm of sites under the strike.
    assert!((host.element_conc_max("Cu") - 8.0).abs() < 1e-9);
    let split = host.treated_split(&r.treatments, &|id| id == "Cu");
    let colloid_bound =
        10_000.0 * host.n_site_for_unit("ppm_site") / host.n_site_for_unit("wt_pct_oxide:CuO");
    let got = host.element_conc_max_split("Cu", &split);
    assert!(got < 8.0, "{got}");
    assert!(
        (got - colloid_bound).abs() < 1e-9,
        "{got} vs {colloid_bound}"
    );

    // More copper than the colloid allows is clamped to that bound, not to the blue glass's.
    r.set_amount("Cu", 5.0);
    let (_, w) = resolve(&r, cat()).expect("resolves");
    let (requested, used) = clamped(&w, "Cu").expect("clamped under the strike");
    assert!((requested - 5.0).abs() < 1e-12, "{requested}");
    assert!(
        (used - colloid_bound).abs() < 1e-9,
        "{used} vs {colloid_bound}"
    );
    // Without the strike the same 5 wt% CuO is within the bound.
    r.treatments.clear();
    let (_, w) = resolve(&r, cat()).expect("resolves");
    assert!(clamped(&w, "Cu").is_none(), "{w:?}");
}

#[test]
fn quartz_iron_is_colorless_until_treated_and_its_states_exclude_each_other() {
    let run = |treatments: &[&str]| {
        resolve(&recipe("quartz", &[("Fe", 50.0)], treatments), cat()).expect("resolves")
    };

    let (t, w) = run(&[]);
    assert!(
        t.o_ray.is_empty(),
        "iron quartz is colorless: {:?}",
        t.o_ray
    );
    assert!(!has_relation(&w), "{w:?}");

    // Irradiation makes the amethyst centre (539 nm).
    let (t, _) = run(&["gamma_irradiation"]);
    assert!(peak_near(&t.o_ray, 539.0).is_some(), "{:?}", t.o_ray);

    // Irradiation then the 450 C anneal: the centre is gone, citrine remains, no relation (the
    // removed centre is inactive).
    let (t, w) = run(&["gamma_irradiation", "thermal_anneal_450"]);
    assert!(peak_near(&t.o_ray, 539.0).is_none(), "{:?}", t.o_ray);
    assert!(
        peak_near(&t.o_ray, 295.0).is_some() || peak_near(&t.o_ray, 411.0).is_some(),
        "citrine: {:?}",
        t.o_ray
    );
    assert!(!has_relation(&w), "{w:?}");

    // The 550 C anneal alone: prasiolite Fe2+ (720 nm).
    let (t, w) = run(&["thermal_anneal_550"]);
    assert!(peak_near(&t.o_ray, 720.0).is_some(), "{:?}", t.o_ray);
    assert!(peak_near(&t.o_ray, 539.0).is_none(), "{:?}", t.o_ray);
    assert!(!has_relation(&w), "{w:?}");

    // Irradiated and then annealed at 550 C: the amethyst centre is removed, so it does not
    // exclude the prasiolite iron.
    let (t, w) = run(&["gamma_irradiation", "thermal_anneal_550"]);
    assert!(peak_near(&t.o_ray, 539.0).is_none(), "{:?}", t.o_ray);
    assert!(!has_relation(&w), "{w:?}");

    // Both anneals together are not a violated relation in the data; they just resolve.
    let _ = run(&["thermal_anneal_450", "thermal_anneal_550"]);
}

#[test]
fn two_element_chromophores_bound_the_sum() {
    // Andalusite V3+ / Cr3+: both entered in wt% V2O3, bounded at 0.5 together.
    let andalusite = cat().host("andalusite").expect("andalusite");
    assert_eq!(
        andalusite.element_unit("Cr").as_deref(),
        Some("wt_pct_oxide:V2O3")
    );
    assert_eq!(andalusite.element_unit("V"), andalusite.element_unit("Cr"));

    let (t, w) = resolve(
        &recipe("andalusite", &[("V", 0.4), ("Cr", 0.4)], &[]),
        cat(),
    )
    .expect("resolves");
    for id in ["V", "Cr"] {
        let (requested, used) = clamped(&w, id).unwrap_or_else(|| panic!("{id} clamped: {w:?}"));
        assert!(
            (requested - 0.4).abs() < 1e-12,
            "{id} requested {requested}"
        );
        assert!((used - 0.25).abs() < 1e-9, "{id} used {used}");
    }
    let (at_cap, w_cap) = resolve(
        &recipe("andalusite", &[("V", 0.25), ("Cr", 0.25)], &[]),
        cat(),
    )
    .expect("resolves");
    assert!(!has_clamp(&w_cap), "{w_cap:?}");
    assert_eq!(t.o_ray.len(), at_cap.o_ray.len());
    assert!(!t.o_ray.is_empty(), "V3+ / Cr3+ colors andalusite");
    for (a, b) in t.o_ray.iter().zip(&at_cap.o_ray) {
        assert!((a.center_nm - b.center_nm).abs() < 1e-3);
        assert!(
            (a.peak - b.peak).abs() <= 1e-5 * b.peak.abs(),
            "{a:?} vs {b:?}"
        );
    }

    // Half of the bound in one element alone is within it.
    let (_, w) = resolve(&recipe("andalusite", &[("V", 0.5)], &[]), cat()).expect("resolves");
    assert!(!has_clamp(&w), "{w:?}");

    // The recipe clamp does the same and counts the two changed entries.
    let mut r = recipe("andalusite", &[("V", 0.4), ("Cr", 0.4)], &[]);
    assert_eq!(r.clamp_amounts(cat()), 2);
    assert!((r.amount("V") - 0.25).abs() < 1e-9 && (r.amount("Cr") - 0.25).abs() < 1e-9);
    assert_eq!(r.clamp_amounts(cat()), 0, "already within the bound");

    // The room an editor offers: 0.5 minus the partner's 0.4.
    let room = andalusite
        .summed_room("Cr", &|id| if id == "V" { 0.4 } else { 0.0 })
        .expect("Cr drives V3+ / Cr3+");
    assert!((room - 0.1).abs() < 1e-9, "{room}");
    let none_left = andalusite
        .summed_room("Cr", &|id| if id == "V" { 0.9 } else { 0.0 })
        .expect("Cr drives V3+ / Cr3+");
    assert!(none_left.abs() < 1e-12, "floored at 0: {none_left}");
    let corundum = cat().host("corundum").expect("corundum");
    assert!(corundum.summed_room("Cr", &|_| 0.0).is_none());

    // Tourmaline Cr3+ / V3+ (chrome tourmaline), both in wt% Cr2O3, bounded at 3.0 together.
    let tourmaline = cat().host("tourmaline").expect("tourmaline");
    assert_eq!(tourmaline.element_unit("V"), tourmaline.element_unit("Cr"));
    let (_, w) = resolve(
        &recipe("tourmaline", &[("Cr", 2.0), ("V", 2.0)], &[]),
        cat(),
    )
    .expect("resolves");
    for id in ["Cr", "V"] {
        let (_, used) = clamped(&w, id).unwrap_or_else(|| panic!("{id} clamped: {w:?}"));
        assert!((used - 1.5).abs() < 1e-9, "{id} used {used}");
    }

    // A suspect multi-element chromophore caps nothing: tanzanite's heated V3+ / Ti4+ row
    // (3000 ppm of sites, hidden) leaves the 8000 ppm of the V3+ chromophore in force.
    let tanzanite = cat().host("tanzanite").expect("tanzanite");
    assert!(tanzanite.summed_room("V", &|_| 0.0).is_none());
    let (_, w) = resolve(&recipe("tanzanite", &[("V", 5000.0)], &[]), cat()).expect("resolves");
    assert!(clamped(&w, "V").is_none(), "{w:?}");
}

#[test]
fn untreated_element_bounds_are_unchanged() {
    let pinned = [
        ("corundum", "Cr", 2.0),
        ("glass_silicate", "Cu", 8.0),
        // The rose-quartz Fe2+-Ti4+ pair (500 ppma) is the largest Fe bound of the host.
        ("quartz", "Fe", 500.0),
        ("quartz", "Al", 100.0),
        ("beryl", "Mn", 0.5),
        ("diamond", "N", 3500.0),
        ("andalusite", "V", 0.5),
        ("tourmaline", "Cr", 3.0),
    ];
    for (host, id, expected) in pinned {
        let got = cat().host(host).expect("host").element_conc_max(id);
        assert!(
            (got - expected).abs() < 1e-9,
            "{host} {id}: {got} vs {expected}"
        );
    }

    // The untreated bound is the split-aware bound under the host's own split, bit for bit.
    for host in &cat().hosts {
        for id in cat().selectable_elements(&host.id) {
            let plain = host.element_conc_max(&id);
            let split = host.element_conc_max_split(&id, &host.valence_split);
            assert_eq!(plain.to_bits(), split.to_bits(), "{} {id}", host.id);
        }
    }
}

#[test]
fn treated_split_matches_the_resolver_and_skips_unmet_requirements() {
    let corundum = cat().host("corundum").expect("corundum");
    let anneal = ["reducing_anneal".to_string()];

    // Fe3+ 0.15 loses 80 % to Fe2+ (0.85 -> 0.85 + 0.8 * 0.15).
    let split = corundum.treated_split(&anneal, &|_| true);
    assert!((split["Fe"]["Fe2+"] - 0.97).abs() < 1e-12, "{split:?}");
    assert!((split["Fe"]["Fe3+"] - 0.03).abs() < 1e-12, "{split:?}");

    // The treatment requires Fe and Ti: without Ti it is skipped and the split stays.
    let split = corundum.treated_split(&anneal, &|id| id == "Fe");
    assert_eq!(split, corundum.valence_split);
    // So does a treatment the host does not know, and no treatment at all.
    let unknown = ["no_such_treatment".to_string()];
    assert_eq!(
        corundum.treated_split(&unknown, &|_| true),
        corundum.valence_split
    );
    assert_eq!(
        corundum.treated_split(&[], &|_| true),
        corundum.valence_split
    );

    // The resolver reports the skip with the same rule: no Ti, no reducing anneal.
    let r = recipe("corundum", &[("Fe", 200.0)], &["reducing_anneal"]);
    let (_, w) = resolve(&r, cat()).expect("resolves");
    assert!(
        w.iter()
            .any(|x| matches!(x, ResolveWarning::TreatmentSkipped { .. })),
        "{w:?}"
    );
}

#[test]
fn solver_recipes_respect_summed_caps() {
    // A dark olive green asks for as much V + Cr as the host allows; whatever the search picks,
    // the recipe it returns carries amounts the resolver does not have to scale down.
    let res = solve_physics(cat(), "andalusite", [25.0, -15.0, 20.0], None, 5.0, &[]);
    let total = res.recipe.amount("V") + res.recipe.amount("Cr");
    assert!(total <= 0.5 + 1e-9, "V + Cr = {total} wt% V2O3");
    let (_, w) = resolve(&res.recipe, cat()).expect("resolves");
    assert!(
        !has_clamp(&w),
        "the returned recipe is already within the bounds: {w:?}"
    );
}
