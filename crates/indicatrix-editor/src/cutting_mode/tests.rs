//! Tests for cutting mode's pure model: step order, keys and signatures, the done marks and
//! how they count, the page texts and the index wheel. No window, database or clock.

use super::{
    CuttingPlan, CuttingStep, StepSide, build_plan,
    dial::{IndexWheel, major_step},
    display::{StepPage, progress_fraction, progress_line, state_text, stone_caption},
    progress::{MarkChange, Progress, StepState, next_position, previous_position},
};
use indicatrix_cut_core::{ConstraintTier, Design, PreformSpec, ScheduleMeta, design::TierRef};

fn round_brilliant() -> Design {
    Design::new(
        PreformSpec::block(2.0, 1.0, 2.0),
        ScheduleMeta::standard_round_brilliant(),
        ConstraintTier::standard_round_brilliant(),
    )
}

fn plan_of(design: &Design) -> CuttingPlan {
    let solved = design.solve().expect("every flat tier solves");
    build_plan(design, &solved, &[]).expect("a design with tiers has a plan")
}

fn step_named<'a>(plan: &'a CuttingPlan, name: &str) -> &'a CuttingStep {
    plan.steps
        .iter()
        .find(|step| step.name == name)
        .unwrap_or_else(|| panic!("no step named {name}"))
}

fn codes_of(plan: &CuttingPlan) -> Vec<&str> {
    plan.steps.iter().map(|step| step.code.as_str()).collect()
}

fn step_for(plan: &CuttingPlan, tier: TierRef) -> &CuttingStep {
    plan.steps
        .iter()
        .find(|step| step.tier == tier)
        .expect("every tier has a step")
}

fn apply(progress: &mut Progress, changes: &[MarkChange]) {
    progress.apply(changes);
}

/// Marks the steps at `positions` done, the way the button does.
fn mark_done(progress: &mut Progress, plan: &CuttingPlan, positions: &[usize]) {
    for &position in positions {
        let changes = progress.toggle_done(&plan.steps[position]);
        apply(progress, &changes);
    }
}

// ---- Step order ------------------------------------------------------------------------------

#[test]
fn a_planar_designs_steps_follow_the_sheet_rows() {
    let design = round_brilliant();
    let solved = design.solve().expect("every tier is pinned");
    let sheet = design.cutting_sheet(&solved);
    let plan = build_plan(&design, &solved, &[]).expect("a plan");

    assert_eq!(plan.steps.len(), sheet.rows.len());
    assert_eq!(plan.gear_teeth, 96);
    for (step, row) in plan.steps.iter().zip(&sheet.rows) {
        assert_eq!(step.number, row.sequence);
        assert_eq!(step.code, row.code);
        assert_eq!(step.name, row.name);
        assert_eq!(step.angle_deg, row.angle_deg);
        assert_eq!(step.meet, row.instruction());
        assert_eq!(step.mast, Some(row.mast));
        assert_eq!(step.depth_mm, row.depth_mm);
    }
    let names: Vec<&str> = plan.steps.iter().map(|step| step.name.as_str()).collect();
    assert_eq!(
        names,
        [
            "Girdle",
            "Pavilion Main",
            "Lower Girdle",
            "Culet",
            "Star",
            "Crown Main",
            "Upper Girdle",
            "Table"
        ],
        "a planar design is cut in its cutting order too: pavilion and girdle, crown, table"
    );
    assert_eq!(
        codes_of(&plan),
        ["G1", "P1", "P2", "Culet", "C1", "C2", "C3", "T"]
    );
    let tiers: Vec<TierRef> = plan.steps.iter().map(|step| step.tier).collect();
    assert_eq!(tiers, design.cutting_order());
    assert_eq!(tiers, design.preview_steps());
}

/// The stored order still decides inside a section: the fixture stored bottom-up is cut
/// culet first, the table last, and the steps and codes follow.
#[test]
fn a_reordered_planar_design_changes_the_steps_not_the_rule() {
    let mut design = round_brilliant();
    design.tiers.reverse();
    design.tier_ids.reverse();
    let plan = plan_of(&design);
    assert_eq!(
        codes_of(&plan),
        ["Culet", "P1", "P2", "G1", "C1", "C2", "C3", "T"]
    );
    let tiers: Vec<TierRef> = plan.steps.iter().map(|step| step.tier).collect();
    assert_eq!(tiers, design.cutting_order());
}

#[test]
fn a_design_with_a_concave_tier_follows_the_cutting_order_of_the_sheet() {
    let design = Design::concave_fixture();
    let solved = design.solve().expect("the fixture's flat tiers solve");
    let sheet = design.cutting_sheet(&solved);
    let plan = build_plan(&design, &solved, &[]).expect("a plan");

    let tiers: Vec<TierRef> = plan.steps.iter().map(|step| step.tier).collect();
    assert_eq!(tiers, design.cutting_order());
    assert_eq!(
        tiers,
        design.preview_steps(),
        "the Cut slider walks the same steps"
    );
    assert_eq!(
        plan.steps.len(),
        design.tiers.len() + design.concave_tiers.len()
    );
    for (step, row) in plan.steps.iter().zip(&sheet.rows) {
        assert_eq!(step.number, row.sequence);
        assert_eq!(step.code, row.code);
        // The fixture's flat tiers have no name: the step says `(unnamed)`, the row is empty.
        let expected_name = if row.name.trim().is_empty() {
            "(unnamed)"
        } else {
            row.name.as_str()
        };
        assert_eq!(step.name, expected_name);
        assert_eq!(step.angle_deg, row.angle_deg);
        assert_eq!(step.meet, row.instruction());
    }

    // Pavilion and girdle tiers first, then the groove, the crowns, then the dimple; a concave
    // tier's code continues the P and C counts of the flat tiers.
    let groove = &plan.steps[3];
    assert_eq!(groove.name, "Groove");
    assert_eq!(groove.number, 4);
    assert_eq!(plan.steps[6].name, "Dimple");
    assert_eq!(groove.code, "P3", "two flat pavilion tiers come first");
    assert_eq!(plan.steps[6].code, "C3", "two flat crown tiers come first");
    assert!(
        groove.meet.starts_with("Groove: "),
        "the name leads the instruction: {}",
        groove.meet
    );
    assert_eq!(groove.side, StepSide::Pavilion);
    assert_eq!(plan.steps[6].side, StepSide::Crown);
}

#[test]
fn a_concave_step_carries_its_tool_line_and_no_mast() {
    let design = Design::concave_fixture();
    let plan = plan_of(&design);
    let flat_count = design.tiers.len();

    let codes = design.tier_codes();
    for (position, tier) in design.concave_tiers.iter().enumerate() {
        let step = step_for(&plan, TierRef::Concave(position));
        assert_eq!(step.name, tier.name);
        assert_eq!(step.code, codes.concave[position].code);
        let line = step
            .tool_line
            .as_deref()
            .expect("a concave step has a tool line");
        for field in tier.second_line_fields() {
            assert!(line.contains(&field), "{field:?} missing from {line:?}");
        }
        assert_eq!(step.mast, None);
        assert_eq!(step.depth_mm, None);
        assert_eq!(
            step.table_row,
            flat_count + position,
            "concave rows follow flat rows"
        );
        assert!(
            line.contains("mm"),
            "the fixture has a girdle diameter, so the tool is also given in millimetres"
        );
    }
    for position in 0..design.tiers.len() {
        let step = step_for(&plan, TierRef::Flat(position));
        assert_eq!(step.tool_line, None);
        assert_eq!(step.table_row, position);
        assert!(step.mast.is_some());
    }
}

#[test]
fn sides_follow_the_solvers_blocks_not_the_sign_of_the_angle() {
    let plan = plan_of(&round_brilliant());
    let sides: Vec<StepSide> = plan.steps.iter().map(|step| step.side).collect();
    assert_eq!(
        sides,
        [
            StepSide::Girdle,
            StepSide::Pavilion,
            StepSide::Pavilion,
            StepSide::Pavilion,
            StepSide::Crown,
            StepSide::Crown,
            StepSide::Crown,
            StepSide::Crown,
        ],
        "the culet sits at a sign-negative zero, which is the pavilion"
    );
    assert_eq!(StepSide::Crown.label(), "Crown");
    assert_eq!(StepSide::Pavilion.label(), "Pavilion");
    assert_eq!(StepSide::Girdle.label(), "Girdle");
}

#[test]
fn a_tier_without_indices_is_one_facet_at_position_zero() {
    let plan = plan_of(&round_brilliant());
    let table = step_named(&plan, "Table");
    assert_eq!(table.indices.len(), 1);
    assert_eq!(table.indices[0].text, "0");
    assert_eq!(table.indices[0].value, 0.0);
}

#[test]
fn a_design_without_solved_masts_or_tiers_has_no_plan() {
    let design = round_brilliant();
    assert!(
        build_plan(&design, &[], &[]).is_none(),
        "a stale solve is refused"
    );
    let empty = Design::new(
        PreformSpec::block(2.0, 1.0, 2.0),
        ScheduleMeta::standard_round_brilliant(),
        Vec::new(),
    );
    assert!(build_plan(&empty, &[], &[]).is_none());
}

// ---- Keys ------------------------------------------------------------------------------------

#[test]
fn keys_are_the_stable_tier_ids_and_index_keys_extend_them() {
    let design = Design::concave_fixture();
    let plan = plan_of(&design);

    let mut keys: Vec<&str> = plan.steps.iter().map(|step| step.key.as_str()).collect();
    keys.sort_unstable();
    keys.dedup();
    assert_eq!(keys.len(), plan.steps.len(), "every step has its own key");

    for position in 0..design.tiers.len() {
        let id = design
            .tier_id_at(position)
            .expect("a built design has tier ids");
        assert_eq!(
            step_for(&plan, TierRef::Flat(position)).key,
            format!("t{}", id.value())
        );
    }
    for (position, id) in design.concave_tier_ids.iter().enumerate() {
        assert_eq!(
            step_for(&plan, TierRef::Concave(position)).key,
            format!("c{}", id.value())
        );
    }
    for step in &plan.steps {
        for index in &step.indices {
            assert_eq!(index.key, format!("{}#{}", step.key, index.text));
        }
    }
}

#[test]
fn a_moved_or_renamed_tier_keeps_its_key() {
    let design = round_brilliant();
    let before = plan_of(&design);
    let star_key = step_named(&before, "Star").key.clone();

    let mut moved = design.clone();
    moved.tiers.swap(1, 2);
    moved.tier_ids.swap(1, 2);
    let after = plan_of(&moved);
    assert_eq!(step_named(&after, "Star").key, star_key);
    assert_ne!(
        after.steps[4].key, before.steps[4].key,
        "the order did change: the crown mains are now cut before the stars"
    );

    let mut renamed = design;
    renamed.tiers[1].name = "Big Star".to_owned();
    let after = plan_of(&renamed);
    assert_eq!(step_named(&after, "Big Star").key, star_key);
}

// ---- Signatures ------------------------------------------------------------------------------

fn signature(design: &Design, name: &str) -> String {
    step_named(&plan_of(design), name).signature.clone()
}

#[test]
fn a_signature_is_sixteen_hex_digits_and_the_same_every_time() {
    let design = round_brilliant();
    let first = plan_of(&design);
    let second = plan_of(&design);
    assert_eq!(first, second);
    for step in &first.steps {
        assert_eq!(step.signature.len(), 16);
        assert!(step.signature.bytes().all(|b| b.is_ascii_hexdigit()));
    }
    let mut unique: Vec<&str> = first.steps.iter().map(|s| s.signature.as_str()).collect();
    unique.sort_unstable();
    unique.dedup();
    assert_eq!(
        unique.len(),
        first.steps.len(),
        "different tiers fingerprint differently"
    );
}

#[test]
fn the_signature_changes_with_the_angle() {
    let design = round_brilliant();
    let mut changed = design.clone();
    changed.tiers[5].angle_deg = -40.5;
    assert_ne!(
        signature(&design, "Pavilion Main"),
        signature(&changed, "Pavilion Main")
    );
    assert_eq!(
        signature(&design, "Table"),
        signature(&changed, "Table"),
        "another tier keeps its mark"
    );
}

#[test]
fn the_signature_changes_with_the_indices_but_not_their_order() {
    let design = round_brilliant();
    let mut added = design.clone();
    added.tiers[2].indices.push(3.0);
    assert_ne!(
        signature(&design, "Crown Main"),
        signature(&added, "Crown Main")
    );

    let mut reordered = design.clone();
    reordered.tiers[2].indices.reverse();
    assert_eq!(
        signature(&design, "Crown Main"),
        signature(&reordered, "Crown Main"),
        "the same positions in another order are the same cut"
    );
}

#[test]
fn the_signature_changes_with_the_depth_and_the_cheater_offset() {
    let mut design = Design::concave_fixture();
    let before = plan_of(&design);
    design.girdle_diameter_mm = Some(7.0);
    let after = plan_of(&design);

    let flat = TierRef::Flat(1);
    assert!(
        step_for(&before, flat).depth_mm.is_some(),
        "the fixture has a girdle diameter, so a depth in millimetres"
    );
    assert_ne!(
        step_for(&before, flat).depth_mm,
        step_for(&after, flat).depth_mm
    );
    assert_ne!(
        step_for(&before, flat).signature,
        step_for(&after, flat).signature,
        "a different depth is a different cut"
    );
    assert_ne!(
        step_named(&before, "Groove").signature,
        step_named(&after, "Groove").signature,
        "so is a tool of a different size in millimetres"
    );

    let mut with_offset = round_brilliant();
    let plain = signature(&with_offset, "Star");
    with_offset.cheater_offsets_deg.insert(1, 1.5);
    let plan = plan_of(&with_offset);
    assert_eq!(step_named(&plan, "Star").cheater_offset_deg, Some(1.5));
    assert_ne!(step_named(&plan, "Star").signature, plain);
}

#[test]
fn a_note_a_rename_and_the_instructions_leave_the_signature_alone() {
    let design = round_brilliant();
    let plain = plan_of(&design);

    let mut noted = design.clone();
    noted.tier_notes.insert(2, "  go slowly here  ".to_owned());
    let plan = plan_of(&noted);
    assert_eq!(step_named(&plan, "Crown Main").notes, "go slowly here");
    assert_eq!(
        step_named(&plan, "Crown Main").signature,
        step_named(&plain, "Crown Main").signature
    );

    let mut renamed = design;
    renamed.tiers[2].name = "Crown Mains".to_owned();
    let plan = plan_of(&renamed);
    assert_eq!(
        step_named(&plan, "Crown Mains").signature,
        step_named(&plain, "Crown Main").signature
    );

    let mut fixture = Design::concave_fixture();
    let before = signature(&fixture, "Dimple");
    fixture.concave_tiers[1].instructions = "Stop when the facet is polished".to_owned();
    assert_eq!(signature(&fixture, "Dimple"), before);
    fixture.concave_tiers[1].displacement[2] = 0.05;
    assert_ne!(signature(&fixture, "Dimple"), before, "the tool moved");
}

// ---- Progress --------------------------------------------------------------------------------

#[test]
fn with_nothing_marked_cutting_mode_opens_at_the_first_step() {
    let plan = plan_of(&round_brilliant());
    let progress = Progress::default();
    assert_eq!(progress.done_count(&plan.steps), 0);
    assert_eq!(progress.first_open(&plan.steps), Some(0));
    assert_eq!(progress.resume_position(&plan.steps), 0);
}

#[test]
fn resume_goes_to_the_first_step_not_done() {
    let plan = plan_of(&round_brilliant());
    let mut progress = Progress::default();
    mark_done(&mut progress, &plan, &[0, 1, 2]);
    assert_eq!(progress.done_count(&plan.steps), 3);
    assert_eq!(progress.resume_position(&plan.steps), 3);

    // A gap is resumed at, not skipped over.
    let mut gap = Progress::default();
    mark_done(&mut gap, &plan, &[0, 1, 3]);
    assert_eq!(gap.resume_position(&plan.steps), 2);
}

#[test]
fn with_every_step_done_resume_stays_on_the_last_step() {
    let plan = plan_of(&round_brilliant());
    let mut progress = Progress::default();
    let all: Vec<usize> = (0..plan.steps.len()).collect();
    mark_done(&mut progress, &plan, &all);
    assert_eq!(progress.done_count(&plan.steps), plan.steps.len());
    assert_eq!(progress.first_open(&plan.steps), None);
    assert_eq!(progress.resume_position(&plan.steps), plan.steps.len() - 1);
    assert_eq!(progress.resume_position(&[]), 0);
}

#[test]
fn a_mark_whose_step_changed_does_not_count_and_is_resumed_at() {
    let design = round_brilliant();
    let plan = plan_of(&design);
    let mut progress = Progress::default();
    mark_done(&mut progress, &plan, &[0, 1, 2, 3]);
    assert_eq!(progress.done_count(&plan.steps), 4);

    // The cutter changes the second step's tier (the pavilion main, stored fifth) after
    // marking it.
    let mut edited = design;
    edited.tiers[5].angle_deg = -40.5;
    let now = plan_of(&edited);

    assert_eq!(progress.state(&now.steps[0]), StepState::Done);
    assert_eq!(progress.state(&now.steps[1]), StepState::Changed);
    assert_eq!(progress.state(&now.steps[4]), StepState::NotDone);
    assert_eq!(
        progress.done_count(&now.steps),
        3,
        "the changed step is not done"
    );
    assert_eq!(progress.changed_count(&now.steps), 1);
    assert_eq!(progress.resume_position(&now.steps), 1);

    // Marking it again takes the new values.
    let changes = progress.mark_done(&now.steps[1]);
    apply(&mut progress, &changes);
    assert_eq!(progress.state(&now.steps[1]), StepState::Done);
    assert_eq!(progress.done_count(&now.steps), 4);
    assert_eq!(progress.changed_count(&now.steps), 0);
}

#[test]
fn the_button_marks_and_takes_the_mark_back_with_the_ticks() {
    let plan = plan_of(&round_brilliant());
    let step = &plan.steps[2];
    let mut progress = Progress::default();

    let mark = progress.toggle_done(step);
    apply(&mut progress, &mark);
    assert_eq!(progress.state(step), StepState::Done);
    assert!((0..step.indices.len()).all(|chip| progress.chip_ticked(step, chip)));

    let again = progress.toggle_done(step);
    apply(&mut progress, &again);
    assert_eq!(progress, Progress::default(), "nothing is left behind");
}

#[test]
fn the_key_marks_only_once() {
    let plan = plan_of(&round_brilliant());
    let step = &plan.steps[0];
    let mut progress = Progress::default();
    let first = progress.mark_done(step);
    assert_eq!(first.len(), 1);
    apply(&mut progress, &first);
    assert!(
        progress.mark_done(step).is_empty(),
        "pressing it again takes nothing back"
    );
    assert_eq!(progress.state(step), StepState::Done);
}

#[test]
fn ticking_every_index_marks_the_step_and_unticking_one_reopens_it() {
    let plan = plan_of(&round_brilliant());
    let step = step_named(&plan, "Crown Main");
    assert_eq!(step.indices.len(), 8);
    let mut progress = Progress::default();

    for chip in 0..7 {
        let changes = progress.toggle_chip(step, chip);
        apply(&mut progress, &changes);
        assert_eq!(
            progress.state(step),
            StepState::NotDone,
            "after {} ticks",
            chip + 1
        );
        assert!(progress.chip_ticked(step, chip));
    }
    assert!(!progress.chip_ticked(step, 7));
    let last = progress.toggle_chip(step, 7);
    apply(&mut progress, &last);
    assert_eq!(
        progress.state(step),
        StepState::Done,
        "the last tick completes it"
    );

    // Taking one tick back reopens the step but keeps the other seven.
    let off = progress.toggle_chip(step, 3);
    apply(&mut progress, &off);
    assert_eq!(progress.state(step), StepState::NotDone);
    for chip in 0..8 {
        assert_eq!(progress.chip_ticked(step, chip), chip != 3, "chip {chip}");
    }
    assert_eq!(progress.done_count(&plan.steps), 0);
}

#[test]
fn a_tick_made_before_the_step_changed_is_not_kept() {
    let design = round_brilliant();
    let plan = plan_of(&design);
    let step = step_named(&plan, "Crown Main");
    let mut progress = Progress::default();
    let changes = progress.toggle_chip(step, 0);
    apply(&mut progress, &changes);
    assert!(progress.chip_ticked(step, 0));

    let mut edited = design;
    edited.tiers[2].angle_deg = 35.0;
    let now = plan_of(&edited);
    assert!(!progress.chip_ticked(step_named(&now, "Crown Main"), 0));
}

#[test]
fn a_chip_past_the_last_index_changes_nothing() {
    let plan = plan_of(&round_brilliant());
    let progress = Progress::default();
    // The table lists no index, so it has exactly one chip (position 0); chip 5 is past it.
    let table = step_named(&plan, "Table");
    assert_eq!(table.indices.len(), 1);
    assert_eq!(progress.toggle_chip(table, 5), Vec::new());
    assert!(!progress.chip_ticked(table, 5));
}

#[test]
fn reset_removes_every_mark() {
    let plan = plan_of(&round_brilliant());
    let mut progress = Progress::from_marks([("t1".to_owned(), "abc".to_owned())]);
    mark_done(&mut progress, &plan, &[0, 1]);
    progress.clear();
    assert_eq!(progress, Progress::default());
    assert_eq!(progress.done_count(&plan.steps), 0);
}

#[test]
fn stepping_stops_at_both_ends() {
    assert_eq!(next_position(0, 3), Some(1));
    assert_eq!(next_position(2, 3), None);
    assert_eq!(next_position(0, 0), None);
    assert_eq!(previous_position(2), Some(1));
    assert_eq!(previous_position(0), None);
}

#[test]
fn state_codes_are_what_the_slint_side_expects() {
    assert_eq!(StepState::NotDone.code(), 0);
    assert_eq!(StepState::Done.code(), 1);
    assert_eq!(StepState::Changed.code(), 2);
}

// ---- Page texts ------------------------------------------------------------------------------

#[test]
fn a_page_reads_like_the_cutting_sheet_row() {
    let plan = plan_of(&round_brilliant());
    let progress = Progress::default();
    // The crown main is the sixth step: pavilion and girdle first, then the star.
    let page = StepPage::new(&plan.steps, 5, &progress).expect("a page");

    assert_eq!(page.heading, "Step 6 of 8");
    assert_eq!(page.tier_name, "C2", "the heading is the tier's code");
    assert_eq!(
        page.meet, "Crown Main: Set to mast depth 0.5900",
        "the tier's name leads the instruction"
    );
    assert_eq!(page.side, "Crown");
    assert_eq!(page.angle, "34.50°");
    assert_eq!(page.chips.len(), 8);
    assert_eq!(page.chips[0].text, "0");
    assert!(page.chips.iter().all(|chip| !chip.ticked));
    assert_eq!(
        page.depth, "Mast 0.5900",
        "no girdle diameter, so no millimetres"
    );
    assert_eq!(page.cheater, "");
    assert_eq!(page.tool, "");
    assert_eq!(page.state, StepState::NotDone);
    assert_eq!(page.state_text, "");
    assert!(page.has_previous && page.has_next);
}

#[test]
fn a_page_shows_depth_in_millimetres_the_cheater_offset_and_the_tool() {
    let mut design = Design::concave_fixture();
    design.cheater_offsets_deg.insert(1, 1.5);
    let plan = plan_of(&design);
    let progress = Progress::default();

    let flat = plan
        .steps
        .iter()
        .position(|step| step.cheater_offset_deg.is_some())
        .expect("the offset tier has a step");
    let page = StepPage::new(&plan.steps, flat, &progress).expect("a page");
    assert!(page.depth.starts_with("Depth ") && page.depth.contains(" mm  ·  mast "));
    assert_eq!(page.cheater, "Cheater offset +1.50°");

    let groove = plan
        .steps
        .iter()
        .position(|step| step.name == "Groove")
        .expect("groove");
    let page = StepPage::new(&plan.steps, groove, &progress).expect("a page");
    assert_eq!(page.depth, "", "a tool cut has no mast");
    assert!(page.tool.starts_with("Tool CYL"), "{}", page.tool);
}

#[test]
fn the_stone_caption_names_the_tier_by_its_code() {
    let plan = plan_of(&round_brilliant());
    assert_eq!(
        stone_caption(&plan.steps[1], plan.steps.len()),
        "The stone after step 2 of 8, P1 highlighted"
    );
    assert_eq!(
        stone_caption(&plan.steps[7], plan.steps.len()),
        "The stone after step 8 of 8, T highlighted"
    );
}

#[test]
fn a_page_says_done_and_changed() {
    let design = round_brilliant();
    let plan = plan_of(&design);
    let mut progress = Progress::default();
    mark_done(&mut progress, &plan, &[0, 1]);

    let done = StepPage::new(&plan.steps, 0, &progress).expect("a page");
    assert_eq!(done.state, StepState::Done);
    assert_eq!(done.state_text, "Done");
    assert!(done.chips.iter().all(|chip| chip.ticked));
    assert!(!done.has_previous);

    let mut edited = design;
    edited.tiers[5].angle_deg = -40.5;
    let now = plan_of(&edited);
    let changed = StepPage::new(&now.steps, 1, &progress).expect("a page");
    assert_eq!(changed.state, StepState::Changed);
    assert_eq!(changed.state_text, "Changed since you marked it");
    assert!(changed.chips.iter().all(|chip| !chip.ticked));
    assert_eq!(state_text(StepState::NotDone), "");

    let last = StepPage::new(&now.steps, now.steps.len() - 1, &progress).expect("a page");
    assert!(!last.has_next && last.has_previous);
    assert!(StepPage::new(&now.steps, now.steps.len(), &progress).is_none());
}

#[test]
fn the_progress_line_counts_done_and_changed_steps() {
    let design = round_brilliant();
    let plan = plan_of(&design);
    let mut progress = Progress::default();
    assert_eq!(progress_line(&plan.steps, &progress), "0 of 8 steps done");
    assert_eq!(progress_fraction(&plan.steps, &progress), 0.0);

    mark_done(&mut progress, &plan, &[0, 1, 2, 3]);
    assert_eq!(progress_line(&plan.steps, &progress), "4 of 8 steps done");
    assert_eq!(progress_fraction(&plan.steps, &progress), 0.5);

    // The second step is the pavilion main (stored fifth), the third the lower girdle (stored
    // seventh).
    let mut edited = design;
    edited.tiers[5].angle_deg = -40.5;
    let now = plan_of(&edited);
    assert_eq!(
        progress_line(&now.steps, &progress),
        "3 of 8 steps done  ·  1 step changed since you marked it"
    );
    edited.tiers[6].angle_deg = -43.0;
    let now = plan_of(&edited);
    assert_eq!(
        progress_line(&now.steps, &progress),
        "2 of 8 steps done  ·  2 steps changed since you marked them"
    );
    assert_eq!(progress_fraction(&[], &progress), 0.0);
}

// ---- Index wheel -----------------------------------------------------------------------------

#[test]
fn the_wheel_puts_position_zero_at_the_top_and_counts_clockwise() {
    let plan = plan_of(&round_brilliant());
    let progress = Progress::default();
    // Main facets sit at 0, 12, 24, ... on the 96-tooth gear.
    let wheel = IndexWheel::new(plan.gear_teeth, step_named(&plan, "Crown Main"), &progress);
    assert_eq!(wheel.marks.len(), 8);

    let top = &wheel.marks[0];
    assert!((top.x - 0.5).abs() < 1e-5, "{}", top.x);
    assert!((top.y - 0.12).abs() < 1e-5, "{}", top.y);
    // Position 24 of 96 is a quarter turn: 3 o'clock.
    let right = &wheel.marks[2];
    assert!((right.x - 0.88).abs() < 1e-5, "{}", right.x);
    assert!((right.y - 0.5).abs() < 1e-5, "{}", right.y);
    // Position 48 is the bottom.
    let bottom = &wheel.marks[4];
    assert!((bottom.x - 0.5).abs() < 1e-5 && (bottom.y - 0.88).abs() < 1e-5);
    assert_eq!(top.label, "0");
    assert_eq!(wheel.marks[1].label, "12");
}

#[test]
fn the_wheel_draws_ticks_spokes_and_the_scale() {
    let plan = plan_of(&round_brilliant());
    let step = step_named(&plan, "Crown Main");
    let wheel = IndexWheel::new(96, step, &Progress::default());

    // Every sixth tooth is major (16 arcs), the other 80 are minor.
    assert_eq!(wheel.major_ticks.matches('M').count(), 16);
    assert_eq!(wheel.minor_ticks.matches('M').count(), 80);
    assert_eq!(wheel.spokes.matches('M').count(), step.indices.len());
    assert_eq!(wheel.scale.len(), 16);
    assert_eq!(wheel.scale[1].label, "6");
}

#[test]
fn the_wheel_marks_ticked_indices_and_drops_labels_when_crowded() {
    let plan = plan_of(&round_brilliant());
    let step = step_named(&plan, "Girdle");
    assert_eq!(step.indices.len(), 16);
    let mut progress = Progress::default();
    let changes = progress.toggle_chip(step, 2);
    apply(&mut progress, &changes);

    let wheel = IndexWheel::new(96, step, &progress);
    assert_eq!(wheel.marks.len(), 16);
    assert!(wheel.marks[2].ticked && !wheel.marks[0].ticked);
    assert!(
        wheel.marks.iter().all(|mark| !mark.label.is_empty()),
        "16 are still labelled"
    );

    let mut many = step.clone();
    many.indices.push(many.indices[0].clone());
    let wheel = IndexWheel::new(96, &many, &Progress::default());
    assert!(
        wheel.marks.iter().all(|mark| mark.label.is_empty()),
        "17 are not"
    );
}

#[test]
fn the_wheel_handles_odd_gears() {
    assert_eq!(major_step(96), 6);
    assert_eq!(major_step(64), 4);
    assert_eq!(major_step(80), 5);
    assert_eq!(major_step(72), 6);
    assert_eq!(major_step(97), 8, "a prime has no even arcs");
    assert_eq!(major_step(5), 1);

    let plan = plan_of(&round_brilliant());
    let empty = IndexWheel::new(0, &plan.steps[0], &Progress::default());
    assert!(empty.marks.is_empty() && empty.minor_ticks.is_empty() && empty.scale.is_empty());

    // A fine gear draws every few teeth only, never more than 192 minor ticks.
    let fine = IndexWheel::new(360, &plan.steps[0], &Progress::default());
    assert!(fine.minor_ticks.matches('M').count() <= 192);

    // A position past the gear wraps round, as the ring does.
    let mut wrapped = plan.steps[0].clone();
    wrapped.indices[0].value = 96.0;
    let wheel = IndexWheel::new(96, &wrapped, &Progress::default());
    assert!((wheel.marks[0].x - 0.5).abs() < 1e-5);
}
