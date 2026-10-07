use super::*;

fn cat() -> &'static ChromophoreCatalogue {
    ChromophoreCatalogue::global()
}

fn ruby_state() -> PhysicsState {
    let mut s = PhysicsState::open("", [0.0; 3], "Ruby", 0.0, cat(), 0);
    s.set_mode(true, None, cat());
    // The initial-solve job is not run in these tests; clear the in-flight flag.
    s.solving = false;
    s
}

/// A host with several colored end members (the pyrope/almandine/spessartine garnet host):
/// the catalogue's own data pick it, no id is named.
fn garnet_host() -> &'static HostData {
    cat()
        .hosts
        .iter()
        .find(|h| {
            let selectable = cat().selectable_elements(&h.id);
            h.end_members
                .iter()
                .filter(|m| !m.colorless && selectable.contains(&m.id))
                .count()
                >= 2
        })
        .expect("the catalogue has an end-member host with several colored end members")
}

#[test]
fn log_slider_maps_bounds_and_round_trips() {
    for max in [0.5, 1.0, 2000.0] {
        assert_eq!(pos_to_amount(0.0, max), 0.0);
        assert!((pos_to_amount(1.0, max) - max).abs() < 1e-9 * max);
        assert_eq!(amount_to_pos(0.0, max), 0.0);
        assert!((amount_to_pos(max, max) - 1.0).abs() < 1e-12);
        let mut last = 0.0;
        for i in 1..=20 {
            let pos = f64::from(i) / 20.0;
            let amount = pos_to_amount(pos, max);
            assert!(amount > last, "monotonic");
            assert!((amount_to_pos(amount, max) - pos).abs() < 1e-9);
            last = amount;
        }
    }
    // Hostile input never escapes the range.
    assert_eq!(pos_to_amount(f64::NAN, 1.0), 0.0);
    assert!(pos_to_amount(7.0, 1.0) <= 1.0);
}

#[test]
fn the_host_list_comes_from_the_catalogue() {
    let s = ruby_state();
    let view = s.view(cat());
    let names: Vec<&str> = cat().hosts.iter().map(|h| h.name.as_str()).collect();
    assert_eq!(
        view.host_names
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        names
    );
    assert_eq!(cat().hosts[view.host_index].id, s.host_id);
    assert_eq!(
        s.host_id,
        cat().host_for_material("Ruby").expect("ruby host").id
    );
}

#[test]
fn opening_resets_all_physics_state() {
    let mut s = ruby_state();
    assert!(s.add_item(&cat().selectable_elements(&s.host_id)[0].clone(), cat()));
    s.toggle_lock("x");
    let job = s.pick([200, 20, 30]);
    let again = PhysicsState::open("", [0.0; 3], "Ruby", 0.0, cat(), s.generation);
    assert!(again.generation > job.generation);
    assert!(again.locked.is_empty());
    assert!(again.target.is_none());
    assert!(!again.history.can_undo());
    assert!(!again.solving);
    assert!(!again.is_dirty());
    assert!(!again.is_physics());
}

#[test]
fn adding_an_element_is_undoable_and_restores_exactly() {
    let mut s = ruby_state();
    let before = s.recipe().cloned();
    let id = cat().selectable_elements(&s.host_id)[0].clone();
    assert!(s.add_item(&id, cat()));
    assert!(s.recipe().unwrap().amount(&id) > 0.0);
    assert!(s.undo());
    assert_eq!(s.recipe().cloned(), before);
    assert!(s.redo());
    assert!(s.recipe().unwrap().amount(&id) > 0.0);
}

#[test]
fn a_slider_drag_is_one_undo_step_committed_on_release() {
    let mut s = ruby_state();
    let id = cat().selectable_elements(&s.host_id)[0].clone();
    assert!(s.add_item(&id, cat()));
    let before_drag = s.recipe().cloned().unwrap();
    for pos in [0.2, 0.4, 0.6, 0.8] {
        s.set_amount_pos(&id, pos, cat());
    }
    assert!(s.history.can_undo());
    s.release();
    assert!(s.undo());
    assert_eq!(
        s.recipe().cloned().unwrap(),
        before_drag,
        "one step back undoes the whole drag"
    );
    // Undoing again goes back before the add, not into the middle of the drag.
    assert!(s.undo());
    assert_eq!(
        s.recipe().unwrap().entries.len(),
        0,
        "back before the first add"
    );
}

#[test]
fn the_undo_stack_holds_fifty_entries() {
    let mut s = ruby_state();
    let id = cat().selectable_elements(&s.host_id)[0].clone();
    for i in 0..60 {
        if i % 2 == 0 {
            s.add_item(&id, cat());
        } else {
            s.remove_item(&id, cat());
        }
    }
    let mut steps = 0;
    while s.undo() {
        steps += 1;
    }
    assert_eq!(steps, RecipeHistory::CAPACITY);
}

#[test]
fn recipe_edits_make_the_dialog_dirty_and_undo_cleans_it() {
    let mut s = PhysicsState::open("", [0.0; 3], "Ruby", 0.0, cat(), 0);
    assert!(!s.is_dirty());
    s.set_mode(true, None, cat());
    assert!(s.is_dirty(), "switching to physics is a change");
    s.set_mode(false, None, cat());
    assert!(
        !s.is_dirty(),
        "a pristine blank recipe on a fantasy material is no change"
    );
    s.set_mode(true, None, cat());
    s.solving = false;
    let id = cat().selectable_elements(&s.host_id)[0].clone();
    s.add_item(&id, cat());
    s.set_mode(false, None, cat());
    assert!(
        s.is_dirty(),
        "an edited recipe keeps the dialog dirty even in fantasy mode"
    );
    s.set_mode(true, None, cat());
    assert!(s.undo());
    s.set_mode(false, None, cat());
    assert!(!s.is_dirty());
}

#[test]
fn switching_to_physics_solves_the_current_fantasy_color() {
    let fantasy = [0.2, 2.8, 2.4];
    let mut s = PhysicsState::open("", fantasy, "Ruby", 0.0, cat(), 0);
    let job = s
        .set_mode(true, None, cat())
        .expect("a first switch solves");
    assert_eq!(s.solving_kind, Some(SolveKind::InitialFromFantasy));
    assert_eq!(
        job.target_lab,
        ColorMode::fantasy(fantasy).fantasy_target_lab()
    );
    assert_eq!(job.host, s.host_id);
    assert!(s.solving);
    // A second switch keeps the recipe and does not solve again.
    s.solving = false;
    s.set_mode(false, None, cat());
    assert!(s.set_mode(true, None, cat()).is_none());
}

#[test]
fn saving_in_fantasy_mode_keeps_the_recipe() {
    let mut s = ruby_state();
    let id = cat().selectable_elements(&s.host_id)[0].clone();
    s.add_item(&id, cat());
    s.set_mode(false, Some([0.1, 0.2, 0.3]), cat());
    let json = s.mode_json();
    let mode = ColorMode::from_json(&json).expect("both payloads are saved");
    assert_eq!(mode.active, ActiveColor::Fantasy);
    assert_eq!(mode.fantasy_rgb, [0.1, 0.2, 0.3]);
    assert!(
        mode.last_recipe
            .as_ref()
            .is_some_and(|r| r.amount(&id) > 0.0)
    );
    // Reopening from that JSON restores the parked recipe.
    let reopened = PhysicsState::open(&json, [0.0; 3], "x", 0.0, cat(), 0);
    assert_eq!(reopened.mode, mode);
    assert!(!reopened.is_dirty());
}

#[test]
fn a_plain_fantasy_material_saves_no_recipe() {
    let s = PhysicsState::open("", [0.4, 0.4, 0.4], "x", 0.0, cat(), 0);
    assert_eq!(s.mode_json(), "");
}

#[test]
fn only_the_latest_generation_writes_back() {
    let mut s = ruby_state();
    let first = s.pick([200, 20, 30]);
    let second = s.pick([20, 20, 200]);
    assert!(
        first.cancel.load(Ordering::SeqCst),
        "the older job is cancelled"
    );
    assert!(!second.cancel.load(Ordering::SeqCst));
    let stale = indicatrix_cut_core::material::color::solve_blocking(
        cat(),
        &s.host_id,
        first.target_lab,
        first.reference_path_mm,
        &[],
    );
    assert!(
        !s.finish_solve(first.generation, stale.clone(), cat()),
        "stale result dropped"
    );
    assert!(s.solving, "the newer job is still pending");
    let depth_before = s.history.can_undo();
    assert!(s.finish_solve(second.generation, stale, cat()));
    assert!(!s.solving);
    assert!(
        s.history.can_undo(),
        "a solver write is pushed on the undo stack (was {depth_before})"
    );
}

#[test]
fn locked_entries_travel_with_the_solve_job() {
    let mut s = ruby_state();
    let id = cat().selectable_elements(&s.host_id)[0].clone();
    s.add_item(&id, cat());
    s.toggle_lock(&id);
    let job = s.pick([200, 20, 30]);
    assert_eq!(job.locked.len(), 1);
    assert_eq!(job.locked[0].0, id);
}

#[test]
fn banner_appears_only_for_hosts_with_non_verified_coefficients() {
    for host in &cat().hosts {
        let weak = host
            .chromophores
            .iter()
            .filter(|c| c.is_offered())
            .any(|c| !matches!(c.confidence.as_str(), "verified" | "measured"));
        assert_eq!(host_banner(host).is_some(), weak, "{}", host.id);
    }
}

#[test]
fn treatments_are_only_those_valid_for_the_present_elements() {
    for host in &cat().hosts {
        if host.treatments.is_empty() {
            continue;
        }
        let none = cat().selectable_treatments(&host.id, &[]);
        assert!(none.iter().all(|t| t.requires.is_empty()), "{}", host.id);
        let all: Vec<String> = cat().selectable_elements(&host.id);
        let refs: Vec<&str> = all.iter().map(String::as_str).collect();
        let every = cat().selectable_treatments(&host.id, &refs);
        assert!(every.len() >= none.len());
    }
}

#[test]
fn end_members_count_down_the_budget_and_grey_out_with_a_reason() {
    let host = garnet_host();
    let mut s = PhysicsState::open("", [0.0; 3], "x", 0.0, cat(), 0);
    s.set_mode(true, None, cat());
    s.solving = false;
    assert!(s.select_host(&host.id, cat()));
    let ems: Vec<String> = host
        .end_members
        .iter()
        .filter(|m| !m.colorless)
        .map(|m| m.id.clone())
        .filter(|id| cat().selectable_elements(&host.id).contains(id))
        .collect();
    assert!(
        ems.len() >= 2,
        "a garnet-style host offers several end members"
    );
    let (first, rest) = ems.split_first().expect("non-empty");
    // The first end member takes the whole budget.
    s.recipe_mut(cat()).set_amount(first, 1.0);
    refresh(s.recipe_mut(cat()), cat());
    let view = s.view(cat());
    let opt = view
        .add_options
        .iter()
        .find(|o| &o.id == rest.first().expect("a second end member"))
        .expect("listed");
    assert!(!opt.enabled, "no room left");
    assert!(opt.reason.contains("100"), "{}", opt.reason);
    assert!(!s.add_item(&opt.id, cat()));
}

#[test]
fn a_garnet_mix_prefills_interpolated_ri_and_sg() {
    let host = garnet_host();
    let mut s = PhysicsState::open("", [0.0; 3], "x", 0.0, cat(), 0);
    s.set_mode(true, None, cat());
    s.solving = false;
    s.select_host(&host.id, cat());
    let pure = s.take_prefill().expect("host choice prefills");
    let em = host
        .end_members
        .iter()
        .find(|m| !m.colorless && cat().selectable_elements(&host.id).contains(&m.id))
        .expect("colored end member");
    assert!(s.add_item(&em.id, cat()));
    s.set_amount_pos(&em.id, 1.0, cat());
    let mix = s.take_prefill().expect("an end-member change re-prefills");
    let expected = indicatrix::optics::chromophore::garnet_optics(&[(em.id.as_str(), 1.0)]);
    assert!((f64::from(mix.ri) - expected.0).abs() < 1e-3);
    assert!((f64::from(mix.specific_gravity) - expected.1).abs() < 1e-3);
    assert!(
        (mix.ri - pure.ri).abs() > 1e-4
            || (mix.specific_gravity - pure.specific_gravity).abs() > 1e-4
    );
}

#[test]
fn the_reference_path_defaults_to_one_and_a_half_girdles() {
    assert!((default_reference_path_mm(6.0) - 9.0).abs() < 1e-6);
    assert!((default_reference_path_mm(0.0) - DEFAULT_PATH_MM).abs() < 1e-6);
    let s = PhysicsState::open("", [0.0; 3], "Ruby", 6.0, cat(), 0);
    assert!((s.reference_path_mm() - 9.0).abs() < 1e-6);
    assert!(s.view(cat()).path_hint.contains("equivalent path"));
    let mut s = ruby_state();
    s.set_path(12.0, cat());
    assert!((s.reference_path_mm() - 12.0).abs() < 1e-6);
}

#[test]
fn swatches_come_from_the_stored_resolved_bands() {
    // A recipe whose entries say "nothing" but whose stored bands are a ruby's: rendering uses
    // the stored bands, never a silent re-resolve.
    let mut ruby = ColorRecipe::new("corundum", cat().data_version);
    ruby.set_amount("Cr", 0.3);
    refresh(&mut ruby, cat());
    let mut stored = ColorRecipe::new("corundum", cat().data_version);
    stored.resolved_bands = ruby.resolved_bands.clone();
    let json = ColorMode::physics(stored, [0.0; 3]).to_json();
    let s = PhysicsState::open(&json, [0.0; 3], "x", 0.0, cat(), 0);
    let swatch = s.view(cat()).d65.expect("swatches").unpol;
    let tensor = ruby.resolved_bands.to_tensor();
    let expected = body_colors(&tensor, f64::from(ruby.reference_path_mm), Illuminant::D65)
        .unpolarised
        .srgb;
    assert_eq!(swatch, expected, "the stored bands are what is shown");
    assert_ne!(
        swatch,
        [255, 255, 255],
        "a re-resolve of the empty entries would be colorless"
    );
}

#[test]
fn an_older_data_version_shows_the_badge_and_updating_is_explicit() {
    let mut recipe = ColorRecipe::new("corundum", cat().data_version.saturating_sub(1));
    recipe.set_amount("Cr", 0.3);
    refresh(&mut recipe, cat());
    recipe.data_version = cat().data_version.saturating_sub(1);
    let before = recipe.resolved_bands.clone();
    let json = ColorMode::physics(recipe, [0.0; 3]).to_json();
    let mut s = PhysicsState::open(&json, [0.0; 3], "x", 0.0, cat(), 0);
    let view = s.view(cat());
    assert!(view.data_updated);
    assert_eq!(
        s.recipe().unwrap().resolved_bands,
        before,
        "opening never re-resolves"
    );
    s.update_data(cat());
    assert!(!s.view(cat()).data_updated);
    assert!(s.undo(), "the update is undoable");
}

#[test]
fn fractions_keep_their_sum_when_one_is_dragged() {
    let mut s = ruby_state();
    let ids: Vec<String> = cat()
        .selectable_elements(&s.host_id)
        .into_iter()
        .take(3)
        .collect();
    for id in &ids {
        assert!(s.add_item(id, cat()));
    }
    let first = ids[0].clone();
    s.set_fraction(&first, 0.7, cat());
    let view = s.view(cat());
    let total: f64 = view.rows.iter().map(|r| r.fraction).sum();
    assert!((total - 1.0).abs() < 1e-6, "{total}");
    let row = view.rows.iter().find(|r| r.id == first).expect("row");
    assert!((row.fraction - 0.7).abs() < 0.02, "{}", row.fraction);
}

#[test]
fn rows_carry_confidence_and_sources() {
    let mut s = ruby_state();
    let id = cat().selectable_elements(&s.host_id)[0].clone();
    s.add_item(&id, cat());
    let view = s.view(cat());
    let row = &view.rows[0];
    assert_ne!(row.sources.len(), 0, "the row names its sources");
    assert_ne!(row.confidence.len(), 0, "the row states its confidence");
    assert!(row.amount_text.contains(&row.unit));
}

/// A stored physics material: a ruby recipe with its stored bands and a fantasy color of its own.
fn stored_physics_mode(fantasy_rgb: [f32; 3]) -> ColorMode {
    let mut recipe = ColorRecipe::new("corundum", cat().data_version);
    recipe.set_amount("Cr", 0.3);
    refresh(&mut recipe, cat());
    ColorMode::physics(recipe, fantasy_rgb)
}

#[test]
fn the_color_area_view_follows_the_feature_and_the_mode() {
    // (feature on, physics mode) -> view, plus what each view shows.
    let fantasy = color_section_view(true, false);
    let physics = color_section_view(true, true);
    let fantasy_only = color_section_view(false, false);
    let over_recipe = color_section_view(false, true);
    assert_eq!(fantasy, ColorSectionView::Fantasy);
    assert_eq!(physics, ColorSectionView::Physics);
    assert_eq!(fantasy_only, ColorSectionView::FantasyOnly);
    assert_eq!(over_recipe, ColorSectionView::FantasyOverRecipe);

    // With the feature on: the toggle is there and exactly one of the two sections is.
    assert!(fantasy.shows_mode_toggle() && fantasy.shows_fantasy_section());
    assert!(!fantasy.shows_physics_section() && !fantasy.shows_recipe_note());
    assert!(physics.shows_mode_toggle() && physics.shows_physics_section());
    assert!(!physics.shows_fantasy_section() && !physics.shows_recipe_note());
    // With the feature off: never the toggle or the recipe section, always the Fantasy
    // controls (otherwise a physics material would have nothing editable).
    for view in [fantasy_only, over_recipe] {
        assert!(!view.shows_mode_toggle() && !view.shows_physics_section());
        assert!(view.shows_fantasy_section());
    }
    // The note appears only over a physics recipe.
    assert!(over_recipe.shows_recipe_note() && !fantasy_only.shows_recipe_note());
}

#[test]
fn this_build_decides_the_view_with_its_own_feature_flag() {
    assert_eq!(PHYSICS_COLOR_UI, cfg!(feature = "physical-color"));
    assert_eq!(
        color_section_view(PHYSICS_COLOR_UI, false).shows_mode_toggle(),
        PHYSICS_COLOR_UI
    );
}

#[test]
fn opening_a_physics_material_and_saving_without_a_color_change_keeps_the_json() {
    for fantasy_rgb in [[0.1, 0.2, 0.3], [0.0; 3]] {
        let json = stored_physics_mode(fantasy_rgb).to_json();
        let s = PhysicsState::open(&json, fantasy_rgb, "Ruby", 0.0, cat(), 0);
        assert!(s.is_physics(), "the stored mode is kept as it is");
        assert!(!s.is_dirty(), "opening is no change");
        assert_eq!(
            s.mode_json(),
            json,
            "what Save hands over is what was stored"
        );
        let view = s.view(cat());
        assert!(view.active);
        assert_eq!(view.mode_json, json);
        // Hidden editor: the Fantasy controls plus the note, over the untouched recipe.
        assert_eq!(
            color_section_view(false, view.active),
            ColorSectionView::FantasyOverRecipe
        );
    }
}

#[test]
fn an_untouched_open_hands_back_the_stored_text_verbatim() {
    // Awkward 16-17 digit amounts in a text that is not what `to_json` writes: neither may
    // change while nothing was touched (a re-serialisation could move the last digit).
    for i in 1..=200_u32 {
        let mut recipe = ColorRecipe::new("corundum", cat().data_version);
        recipe.set_amount("Cr", f64::from(i).sqrt() / 7.0);
        let stored = ColorMode::physics(recipe, [0.1, 0.2, 0.3]);
        let text = serde_json::to_string_pretty(&stored).expect("serialises");
        let mut s = PhysicsState::open(&text, [0.1, 0.2, 0.3], "Ruby", 0.0, cat(), 0);
        assert_eq!(s.mode_json(), text, "amount #{i}");
        assert_eq!(s.view(cat()).mode_json, text);
        if i == 1 {
            // A real change is re-serialised; undoing it returns to the stored text.
            s.set_strength(2.0, cat());
            s.release();
            assert_ne!(s.mode_json(), text);
            assert_eq!(s.mode_json(), s.mode.to_json());
            assert!(s.undo());
            assert_eq!(s.mode_json(), text, "back at the opened state");
        }
    }
}

#[test]
fn choosing_a_fixed_color_over_a_recipe_switches_to_fantasy_and_keeps_the_recipe() {
    let stored = stored_physics_mode([0.1, 0.2, 0.3]);
    let json = stored.to_json();
    // "Clear" (all zero) is an explicit pick: it is never re-seeded from the recipe.
    for pick in [[0.0; 3], [0.4, 0.5, 0.6]] {
        let mut s = PhysicsState::open(&json, [0.1, 0.2, 0.3], "Ruby", 0.0, cat(), 0);
        s.choose_fixed_color(Some(pick));
        assert!(!s.is_physics());
        assert!(s.is_dirty(), "a pick is a change");
        assert_eq!(s.mode.fantasy_rgb, pick);
        let saved = ColorMode::from_json(&s.mode_json()).expect("both payloads are saved");
        assert_eq!(saved.active, ActiveColor::Fantasy);
        assert_eq!(saved.fantasy_rgb, pick);
        assert_eq!(
            saved.last_recipe, stored.last_recipe,
            "the recipe stays saved"
        );
        // The note is gone once the color is fixed.
        assert_eq!(
            color_section_view(false, s.view(cat()).active),
            ColorSectionView::FantasyOnly
        );
    }
}

#[test]
fn choosing_keep_over_a_recipe_seeds_a_fixed_color_from_it() {
    let stored = stored_physics_mode([0.0; 3]);
    let mut s = PhysicsState::open(&stored.to_json(), [0.0; 3], "Ruby", 0.0, cat(), 0);
    s.choose_fixed_color(None);
    assert!(!s.is_physics());
    assert_ne!(
        s.mode.fantasy_rgb, [0.0; 3],
        "a payload that never had a color takes the recipe's nearest legacy color"
    );
    assert_eq!(s.mode.last_recipe, stored.last_recipe);
}

#[test]
fn a_free_pick_over_a_recipe_switches_to_fantasy_and_keeps_the_recipe() {
    let stored = stored_physics_mode([0.1, 0.2, 0.3]);
    let mut s = PhysicsState::open(&stored.to_json(), [0.1, 0.2, 0.3], "Ruby", 0.0, cat(), 0);
    s.set_fantasy_pick([0.2, 1.0, 2.0]);
    let saved = ColorMode::from_json(&s.mode_json()).expect("the pick is saved");
    assert_eq!(saved.active, ActiveColor::Fantasy);
    assert_eq!(saved.fantasy_rgb, [0.2, 1.0, 2.0]);
    assert_eq!(saved.last_recipe, stored.last_recipe);
}

#[test]
fn the_data_confidence_banner_carries_no_symbol_glyph() {
    // The warning sign is an icon drawn by the banner: the UI font has no glyph for it.
    for text in cat().hosts.iter().filter_map(host_banner) {
        assert!(!text.contains('\u{26a0}'), "{text}");
        assert!(text.starts_with("Strengths uncalibrated"), "{text}");
    }
}

#[test]
fn a_free_fantasy_pick_is_saved_even_without_a_recipe() {
    let mut s = PhysicsState::open("", [0.0; 3], "x", 0.0, cat(), 0);
    assert_eq!(s.mode_json(), "");
    assert!(!s.is_dirty());
    s.set_fantasy_pick([0.2, 1.0, 2.0]);
    assert!(s.is_dirty());
    let mode = ColorMode::from_json(&s.mode_json()).expect("the pick is saved");
    assert_eq!(mode.fantasy_rgb, [0.2, 1.0, 2.0]);
    assert!(mode.last_recipe.is_none());
    assert_eq!(mode.active, ActiveColor::Fantasy);
}
