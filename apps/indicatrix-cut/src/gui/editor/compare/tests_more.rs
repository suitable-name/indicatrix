#[test]
fn the_figures_of_a_concave_stone_differ_from_its_flat_twin() {
    let session = CompareSession::build(
        input(concave_design(), "concave"),
        input(concave_design_cleared(), "flat"),
        CompareOrigin::Snapshot,
        START_POSE,
    );
    let metrics = super::metrics::SessionMetrics::measure(
        &session,
        indicatrix::optics::LightingPreset::RingLights,
    );
    let strip = metrics.strip();
    assert_eq!(strip.rows.len(), 5, "both sides are measured");
    assert!(
        strip.rows.iter().any(|row| row.before != row.after),
        "the tools change at least one figure: {:?}",
        strip.rows
    );
}

#[test]
fn a_side_whose_concave_tiers_could_not_be_placed_says_so() {
    let mut session = CompareSession::build(
        input(concave_design(), "a"),
        input(round_brilliant(), "b"),
        CompareOrigin::Snapshot,
        START_POSE,
    );
    assert_eq!(
        status_text(Some(&session), Renderer::Solid, TracedProgress::Done, 48),
        "Solid",
        "a placed concave side needs no extra text"
    );
    session.before.tools_dropped = true;
    let status = status_text(Some(&session), Renderer::Solid, TracedProgress::Done, 48);
    assert!(
        status
            .contains("Before: its concave tiers could not be placed, so the flat stone is shown"),
        "{status}"
    );
    assert!(!status.contains("After:"), "{status}");
}

#[test]
fn only_a_slow_concave_side_gets_fewer_samples() {
    // A planar side always traces at full quality, however slow.
    assert_eq!(traced_spp_for(TRACED_SPP, false, Some(30.0)), TRACED_SPP);
    // A concave side does until it has been measured slow.
    assert_eq!(traced_spp_for(TRACED_SPP, true, None), TRACED_SPP);
    assert_eq!(
        traced_spp_for(TRACED_SPP, true, Some(SLOW_TRACED_SIDE_SECS)),
        TRACED_SPP,
        "exactly at the limit is not over it"
    );
    assert_eq!(
        traced_spp_for(TRACED_SPP, true, Some(SLOW_TRACED_SIDE_SECS + 0.1)),
        TRACED_SPP / 2
    );
    assert_eq!(
        traced_spp_for(1, true, Some(99.0)),
        1,
        "never below one sample"
    );
}

#[test]
fn the_status_names_a_reduced_side_and_is_silent_at_full_quality() {
    assert_eq!(reduced_spp_note([TRACED_SPP, TRACED_SPP], TRACED_SPP), None);
    let note = reduced_spp_note([TRACED_SPP, TRACED_SPP / 2], TRACED_SPP).expect("reduced");
    assert!(note.contains("After") && !note.contains("Before"), "{note}");
    assert!(note.contains(&format!("{} spp", TRACED_SPP / 2)), "{note}");
}

// --- the "Compare against" reference picker (Optimize comparison) ---

#[test]
fn the_reference_index_round_trips_and_defaults_to_the_current_design() {
    use super::origins::Reference;
    assert_eq!(Reference::default(), Reference::Current);
    assert_eq!(
        Reference::from_index(Reference::Current.index()),
        Reference::Current
    );
    assert_eq!(
        Reference::from_index(Reference::Original.index()),
        Reference::Original
    );
    assert_eq!(Reference::from_index(7), Reference::Current);
}

#[test]
fn the_before_side_is_the_original_when_picked_and_the_live_design_otherwise() {
    use super::origins::{Reference, pick_before};
    let current = round_brilliant();
    let original = round_brilliant_steeper_crown();
    let held = || Some((original.clone(), "Before retarget to Sapphire".to_string()));

    let (design, label, offer) = pick_before(&current, held(), Reference::Current);
    assert_eq!(design, current);
    assert_eq!(label, "Current Design");
    assert_eq!(
        offer.original_label.as_deref(),
        Some("Before retarget to Sapphire")
    );

    let (design, label, offer) = pick_before(&current, held(), Reference::Original);
    assert_eq!(design, original);
    assert!(label.starts_with("Original"));
    assert_eq!(offer.selected, Reference::Original);
}

#[test]
fn the_picker_is_hidden_and_falls_back_to_current_when_no_original_is_held() {
    use super::origins::{Reference, pick_before, reference_options};
    let current = round_brilliant();
    let (design, label, offer) = pick_before(&current, None, Reference::Original);
    assert_eq!(design, current);
    assert_eq!(label, "Current Design");
    assert!(offer.original_label.is_none());
    assert_eq!(
        reference_options(&offer),
        vec!["Current Design".to_string()]
    );
}

#[test]
fn ray_frames_obey_the_same_generation_gate_and_never_fall_back_to_solid() {
    let mut book = FrameBook::default();
    let first = book.bump();
    assert!(book.accept(FrameKind::Solid, first, Renderer::Ray));
    assert!(book.accept(FrameKind::Traced, first, Renderer::Ray));
    assert!(!book.accept(FrameKind::Solid, first, Renderer::Ray));
    let second = book.bump();
    assert!(!book.accept(FrameKind::Traced, first, Renderer::Ray));
    assert!(!book.accept(FrameKind::Traced, second, Renderer::Solid));
    assert!(book.accept(FrameKind::Traced, second, Renderer::Ray));
}

#[test]
fn ray_status_reports_taps_and_retains_unresolved_material_reasons() {
    let mut session = CompareSession::build(
        input(round_brilliant(), "before"), input(round_brilliant(), "after"),
        CompareOrigin::Snapshot, START_POSE,
    );
    let status = status_text(Some(&session), Renderer::Ray, TracedProgress::Done, 48);
    assert_eq!(status, "Ray-Traced (8 taps)");
    session.before.material = Err("recipe unavailable".to_string());
    assert!(status_text(Some(&session), Renderer::Ray, TracedProgress::Done, 48)
        .contains("Before: material not resolved"));
}

#[test]
fn ray_compare_uses_the_shared_deterministic_renderer() {
    let session = CompareSession::build(
        input(round_brilliant(), "before"), input(round_brilliant(), "after"),
        CompareOrigin::Snapshot, START_POSE,
    );
    let side = &session.before;
    let material = side.material.as_ref().expect("Diamond resolves");
    let pose = session.pose;
    let render = || crate::bridge::preview_render::render_rgba_at_pose_ray(
        &side.stone.planes, &side.stone.tools, &side.stone.frosted, material,
        (pose.yaw, pose.pitch, pose.distance), (16, 16),
        indicatrix::renderer::engine::RayOptions::default(), 16,
    ).expect("Ray-Traced supports Diamond");
    let pixels = render();
    assert_eq!(pixels.len(), 16 * 16 * 4);
    assert_eq!(pixels, render());
}
