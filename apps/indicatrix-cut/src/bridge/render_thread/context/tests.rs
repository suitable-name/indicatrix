//! Tests for `RenderContext`'s plane-ownership arbitration, staleness check, custom
//! specific-gravity lookup, and the material-override resolution in
//! [`super::materials`].

use super::{
    PlanesOwner, RenderContext,
    materials::{resolve_material, resolve_material_with_override},
};
use indicatrix::{geometry::cuts::StandardGemCuts, optics::materials::GemMaterial};
use std::sync::Arc;

// ---- PlanesOwner / claim_active_planes ------------------------

#[test]
fn builtin_is_the_default_owner_and_anything_may_claim_over_it() {
    let ctx = RenderContext::default();
    assert_eq!(ctx.planes_owner, PlanesOwner::Builtin);
    assert!(ctx.may_claim_active_planes(PlanesOwner::Catalogue { entry_id: 1 }));
    assert!(ctx.may_claim_active_planes(PlanesOwner::Editor { generation: 0 }));
}

#[test]
fn a_catalogue_claim_may_overwrite_an_editor_owner() {
    let mut ctx = RenderContext::default();
    assert!(ctx.claim_active_planes(
        Arc::new(Vec::new()),
        None,
        PlanesOwner::Editor { generation: 3 },
    ));
    assert!(
        ctx.claim_active_planes(
            Arc::new(Vec::new()),
            None,
            PlanesOwner::Catalogue { entry_id: 42 },
        ),
        "an explicit Library selection must always be able to show its row"
    );
    assert_eq!(ctx.planes_owner, PlanesOwner::Catalogue { entry_id: 42 });
}

#[test]
fn a_stale_editor_generation_never_overwrites_a_newer_one() {
    let mut ctx = RenderContext::default();
    assert!(ctx.claim_active_planes(
        Arc::new(Vec::new()),
        None,
        PlanesOwner::Editor { generation: 5 },
    ));
    // A background auto-solve started against generation 2 finishes late,
    // after a newer edit (generation 5) already landed -- it must not win.
    assert!(!ctx.claim_active_planes(
        Arc::new(Vec::new()),
        None,
        PlanesOwner::Editor { generation: 2 },
    ));
    assert_eq!(ctx.planes_owner, PlanesOwner::Editor { generation: 5 });
}

#[test]
fn a_newer_or_equal_editor_generation_may_overwrite_the_current_one() {
    let mut ctx = RenderContext::default();
    assert!(ctx.claim_active_planes(
        Arc::new(Vec::new()),
        None,
        PlanesOwner::Editor { generation: 5 },
    ));
    assert!(ctx.claim_active_planes(
        Arc::new(Vec::new()),
        None,
        PlanesOwner::Editor { generation: 5 },
    ));
    assert!(ctx.claim_active_planes(
        Arc::new(Vec::new()),
        None,
        PlanesOwner::Editor { generation: 6 },
    ));
    assert_eq!(ctx.planes_owner, PlanesOwner::Editor { generation: 6 });
}

#[test]
fn two_catalogue_claims_freely_replace_each_other() {
    let mut ctx = RenderContext::default();
    assert!(ctx.claim_active_planes(
        Arc::new(Vec::new()),
        None,
        PlanesOwner::Catalogue { entry_id: 1 },
    ));
    assert!(ctx.claim_active_planes(
        Arc::new(Vec::new()),
        None,
        PlanesOwner::Catalogue { entry_id: 2 },
    ));
    assert_eq!(ctx.planes_owner, PlanesOwner::Catalogue { entry_id: 2 });
}

// ---- claim_active_planes's stone_width_mm warning -----------
//
// The warning itself is a `tracing::warn!` inside `claim_active_planes` -- this crate
// has no test-capturing `tracing` subscriber wired up, so its exact text isn't
// asserted here. These instead pin the CONDITION the warning fires under
// (`stone_width_mm > 0.0` and the owner actually changing) by checking that a
// claim under each circumstance still behaves exactly like the plain ownership
// tests above -- accepted/refused per `may_claim_active_planes`, with
// `stone_width_mm` itself always left untouched, warning or not.

#[test]
fn a_zero_stone_width_claim_is_unaffected_by_an_owner_change() {
    let mut ctx = RenderContext::default();
    assert!(ctx.claim_active_planes(
        Arc::new(Vec::new()),
        None,
        PlanesOwner::Catalogue { entry_id: 1 },
    ));
    assert_eq!(ctx.stone_width_mm, 0.0);
}

#[test]
fn a_nonzero_stone_width_survives_an_owner_change_the_claim_still_accepts() {
    let mut ctx = RenderContext {
        stone_width_mm: 6.5,
        ..RenderContext::default()
    };
    // A catalogue browse taking the slot from `Builtin` is an ordinary accepted
    // claim -- this is exactly the scenario the warning
    // flags, but the claim itself must still succeed and must never reset the
    // control on the caller's behalf (see `stone_width_mm`'s own doc comment for
    // why an automatic reset was not chosen).
    assert!(ctx.claim_active_planes(
        Arc::new(Vec::new()),
        None,
        PlanesOwner::Catalogue { entry_id: 9 },
    ));
    assert_eq!(ctx.stone_width_mm, 6.5);
}

#[test]
fn a_nonzero_stone_width_claim_for_the_same_owner_is_not_an_owner_change() {
    let mut ctx = RenderContext {
        stone_width_mm: 6.5,
        ..RenderContext::default()
    };
    assert!(ctx.claim_active_planes(
        Arc::new(Vec::new()),
        None,
        PlanesOwner::Editor { generation: 1 },
    ));
    // A second claim at the SAME (or a newer) generation is the ordinary
    // "design re-solved" case, not a switch to a different stone -- ordinary
    // `claim_active_planes` behaviour, with the warning above never firing.
    assert!(ctx.claim_active_planes(
        Arc::new(Vec::new()),
        None,
        PlanesOwner::Editor { generation: 2 },
    ));
    assert_eq!(ctx.stone_width_mm, 6.5);
}

// --- traced_planes_are_stale ---

#[test]
fn traced_planes_match_the_generation_they_were_claimed_at() {
    let mut ctx = RenderContext::default();
    assert!(ctx.claim_active_planes(
        Arc::new(Vec::new()),
        None,
        PlanesOwner::Editor { generation: 7 },
    ));
    assert!(!ctx.traced_planes_are_stale(7));
}

#[test]
fn traced_planes_are_stale_once_a_newer_edit_has_landed() {
    let mut ctx = RenderContext::default();
    assert!(ctx.claim_active_planes(
        Arc::new(Vec::new()),
        None,
        PlanesOwner::Editor { generation: 7 },
    ));
    // An edit bumped `EditorState::generation` to 8, but nothing has
    // re-solved/re-claimed the slot yet -- the trace on screen is now for an
    // edit that no longer matches the live design.
    assert!(ctx.traced_planes_are_stale(8));
}

#[test]
fn a_slot_the_editor_has_never_owned_is_never_stale() {
    let ctx = RenderContext::default();
    assert_eq!(ctx.planes_owner, PlanesOwner::Builtin);
    assert!(!ctx.traced_planes_are_stale(1));

    let mut ctx = RenderContext::default();
    assert!(ctx.claim_active_planes(
        Arc::new(Vec::new()),
        None,
        PlanesOwner::Catalogue { entry_id: 3 },
    ));
    assert!(!ctx.traced_planes_are_stale(1));
}

#[test]
fn a_refused_claim_leaves_every_field_untouched() {
    let mut ctx = RenderContext::default();
    let original_planes = Arc::new(StandardGemCuts::emerald_cut());
    ctx.active_planes = Arc::clone(&original_planes);
    ctx.design_gear = Some((96, 0.5));
    ctx.planes_owner = PlanesOwner::Editor { generation: 10 };

    // Only a superseded editor write is refused (a Library selection may take the
    // slot over, see `may_claim_active_planes`): an auto-solve from generation 9
    // finishing after the user's edit at generation 10.
    let accepted = ctx.claim_active_planes(
        Arc::new(StandardGemCuts::standard_round_brilliant()),
        Some((64, 0.0)),
        PlanesOwner::Editor { generation: 9 },
    );

    assert!(!accepted);
    assert!(Arc::ptr_eq(&ctx.active_planes, &original_planes));
    assert_eq!(ctx.design_gear, Some((96, 0.5)));
    assert_eq!(ctx.planes_owner, PlanesOwner::Editor { generation: 10 });
}

// ---- custom_specific_gravity ---------------------------------

#[test]
fn default_context_has_no_custom_specific_gravity_entries() {
    let ctx = RenderContext::default();
    assert!(ctx.custom_material_specific_gravity.is_empty());
    assert_eq!(ctx.custom_specific_gravity("My Garnet"), None);
}

#[test]
fn custom_specific_gravity_finds_a_recorded_entry_case_insensitively() {
    let ctx = RenderContext {
        custom_material_specific_gravity: Arc::new(vec![("My Garnet".to_string(), 3.9)]),
        ..RenderContext::default()
    };
    assert_eq!(ctx.custom_specific_gravity("my garnet"), Some(3.9));
    assert_eq!(ctx.custom_specific_gravity("MY GARNET"), Some(3.9));
}

#[test]
fn custom_specific_gravity_is_none_for_an_unrecorded_name() {
    let ctx = RenderContext {
        custom_material_specific_gravity: Arc::new(vec![("My Garnet".to_string(), 3.9)]),
        ..RenderContext::default()
    };
    assert_eq!(ctx.custom_specific_gravity("Custom Diamond"), None);
}

// ---- resolve_material_with_override -----------------------

#[test]
fn no_override_falls_through_to_the_plain_by_name_lookup() {
    let materials = GemMaterial::all_materials();
    let by_name = resolve_material(&materials, &[], "Diamond").expect("Diamond resolves");
    let via_override_fn =
        resolve_material_with_override(&materials, &[], None, "Diamond").expect("Diamond resolves");
    assert_eq!(by_name.name, via_override_fn.name);
    assert_eq!(by_name.dispersion, via_override_fn.dispersion);
}

#[test]
fn an_override_wins_regardless_of_what_material_name_says() {
    let materials = GemMaterial::all_materials();
    let quartz = GemMaterial::new_custom("Quartz-ish", 1.5442, 0.013, 0.0, [0.0, 0.0, 0.0]);
    let resolved = resolve_material_with_override(
        &materials,
        &[],
        Some(&quartz),
        "Diamond", // the stale/fallback name a design with no real material carries
    )
    .expect("an explicit override always resolves");
    assert_eq!(resolved.name, quartz.name);
    assert_eq!(resolved.dispersion, quartz.dispersion);
}

#[test]
fn an_unrecognized_name_refuses_rather_than_substituting_diamond() {
    // The whole point of `resolve_material` dropping its old `materials[0]`
    // fallback -- see that function's own doc comment. A design naming a
    // deleted custom material, or an unrecognized/typo'd name, must be told
    // apart from a real, resolved Diamond.
    let materials = GemMaterial::all_materials();
    assert_eq!(
        resolve_material(&materials, &[], "no such stone"),
        None,
        "an unrecognized name must refuse, not silently resolve to Diamond"
    );
    assert_eq!(
        resolve_material_with_override(&materials, &[], None, "no such stone"),
        None,
        "with no override, the by-name refusal must still propagate"
    );
}

// ---- Final-picture live transfer ---------------------------------------------------

/// A final-picture epoch turns `Both` into `RemoteOnly` (local pauses after the
/// handoff, the remote's display frames are the image); nothing else changes, and a
/// release clears the flag with the rest of the remote state.
#[test]
fn a_display_only_epoch_acts_as_remote_only_until_released() {
    use super::effective_live_target;
    use crate::settings::LiveComputeTarget::{Both, LocalOnly, RemoteOnly};
    assert_eq!(effective_live_target(Both, true), RemoteOnly);
    assert_eq!(effective_live_target(Both, false), Both);
    assert_eq!(effective_live_target(RemoteOnly, true), RemoteOnly);
    assert_eq!(effective_live_target(LocalOnly, true), LocalOnly);

    let mut ctx = RenderContext {
        live_display_only: true,
        remote_active: true,
        ..RenderContext::default()
    };
    assert_eq!(ctx.effective_live_target(), RemoteOnly);
    ctx.release_remote();
    assert!(!ctx.live_display_only);
    assert_eq!(ctx.effective_live_target(), Both);
}

#[test]
fn the_editors_material_survives_a_library_selection_and_comes_back() {
    let mut ctx = RenderContext::default();
    assert!(ctx.claim_active_planes(
        Arc::new(Vec::new()),
        None,
        PlanesOwner::Editor { generation: 1 },
    ));
    "Sapphire".clone_into(&mut ctx.material_name);
    assert!(ctx.claim_active_planes(
        Arc::new(Vec::new()),
        None,
        PlanesOwner::Catalogue { entry_id: 7 },
    ));
    // The row resolves no material and refuses, as a Library selection may.
    ctx.material_name.clear();
    ctx.material_unresolved = Some("no material".to_string());
    assert!(ctx.claim_active_planes(
        Arc::new(Vec::new()),
        None,
        PlanesOwner::Editor { generation: 2 },
    ));
    assert_eq!(ctx.material_name, "Sapphire");
    assert!(ctx.material_unresolved.is_none());
    assert!(ctx.editor_material_stash.is_none());
}

// ---- claim_active_geometry: planes and concave tools move together -------------

fn a_ball() -> Arc<Vec<indicatrix::geometry::tool::ToolPrimitive>> {
    Arc::new(vec![indicatrix::geometry::tool::ToolPrimitive::ball(
        glam::Vec3::new(0.0, 0.3, 0.0),
        0.1,
    )])
}

#[test]
fn a_geometry_claim_replaces_planes_tools_and_placements_together() {
    let mut ctx = RenderContext::default();
    assert!(ctx.active_tools.is_empty(), "a default stone is convex");
    let tools = a_ball();
    assert!(ctx.claim_active_geometry(
        Arc::new(StandardGemCuts::emerald_cut()),
        Arc::clone(&tools),
        vec![(1, 0)],
        None,
        PlanesOwner::Editor { generation: 1 },
    ));
    assert!(Arc::ptr_eq(&ctx.active_tools, &tools));
    assert_eq!(ctx.active_placements, vec![(1, 0)]);
}

#[test]
fn a_planes_only_claim_clears_the_previous_designs_tools() {
    let mut ctx = RenderContext::default();
    assert!(ctx.claim_active_geometry(
        Arc::new(StandardGemCuts::emerald_cut()),
        a_ball(),
        vec![(0, 0)],
        None,
        PlanesOwner::Editor { generation: 1 },
    ));
    // A catalogue row knows no tools: its stone is the flat one, not the editor's
    // design with the previous tool still cut into it.
    assert!(ctx.claim_active_planes(
        Arc::new(StandardGemCuts::standard_round_brilliant()),
        None,
        PlanesOwner::Catalogue { entry_id: 5 },
    ));
    assert!(ctx.active_tools.is_empty());
    assert!(ctx.active_placements.is_empty());
}

#[test]
fn a_refused_geometry_claim_leaves_the_tools_untouched() {
    let mut ctx = RenderContext::default();
    let tools = a_ball();
    assert!(ctx.claim_active_geometry(
        Arc::new(StandardGemCuts::emerald_cut()),
        Arc::clone(&tools),
        vec![(0, 0)],
        None,
        PlanesOwner::Editor { generation: 9 },
    ));
    assert!(!ctx.claim_active_geometry(
        Arc::new(StandardGemCuts::standard_round_brilliant()),
        Arc::new(Vec::new()),
        Vec::new(),
        None,
        PlanesOwner::Editor { generation: 3 },
    ));
    assert!(Arc::ptr_eq(&ctx.active_tools, &tools));
    assert_eq!(ctx.active_placements, vec![(0, 0)]);
}

#[test]
fn re_claiming_a_planar_stone_keeps_the_same_empty_tool_arc_so_the_scene_identity_holds() {
    let mut ctx = RenderContext::default();
    let before = Arc::clone(&ctx.active_tools);
    // Every editor refresh claims again; a planar design must not look like a new scene
    // each time because the empty tool list was reallocated.
    assert!(ctx.claim_active_geometry(
        Arc::new(StandardGemCuts::emerald_cut()),
        Arc::new(Vec::new()),
        Vec::new(),
        None,
        PlanesOwner::Editor { generation: 1 },
    ));
    assert!(Arc::ptr_eq(&ctx.active_tools, &before));
}
