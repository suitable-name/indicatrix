//! Tests of the stored plans against an in-memory library: the whole path from a planned
//! layout to the table and back, renaming, export and import, the saved list, and
//! opening a plan made in another library.

use super::{
    convert::CandidateSource,
    dto::SavedDesignDto,
    fixtures::{plain_block, sample_layout, write},
    format::{
        RankedLayout, SerializeInput, design_shape, parse_and_validate_plan, payload_with_name,
        plan_summary, serialize_checked_plan,
    },
    open::{check_staleness, prepare_open},
    store,
};
use crate::gui::rough_plan::run::DesignStatus;
use indicatrix_cut_core::rough_plan::{
    CandidateDesign, PlanInput, PlanSettings, RoughBase, RoughCut, RoughLayout, RoughModel,
    plan::plan,
};
use indicatrix_vault::{
    db::sqlite::Database,
    model::{
        entry::FacetingDiagramEntry,
        solid_extents::{SolidExtents, SolidExtentsSource},
    },
};
use std::sync::Mutex;

/// An empty in-memory library.
pub(super) fn library() -> Mutex<Database> {
    Mutex::new(Database::new(Some(":memory:")).expect("an in-memory library opens"))
}

/// Every float of `layout` as bits, so two layouts compare exactly (`-0.0` and `0.0`
/// differ, a NaN equals itself).
fn bits(layout: &RoughLayout) -> Vec<u64> {
    let mut out = vec![
        layout.total_carat.to_bits(),
        layout.total_volume_mm3.to_bits(),
        layout.yield_fraction.to_bits(),
    ];
    for slab in &layout.cut_plan.slabs {
        out.push(slab.thickness_mm.to_bits());
        for bar in &slab.bars {
            out.push(bar.width_mm.to_bits());
            out.extend(bar.pieces_mm.iter().map(|piece| piece.to_bits()));
        }
    }
    for stone in &layout.stones {
        let scalars = [stone.carat, stone.volume_mm3, stone.pose.mm_per_unit];
        out.extend(
            stone
                .piece_origin_mm
                .iter()
                .chain(&stone.piece_size_mm)
                .chain(&stone.stone_size_mm)
                .chain(&scalars)
                .chain(&stone.pose.center_mm)
                .chain(stone.pose.axes.iter().flatten())
                .map(|value| value.to_bits()),
        );
    }
    out
}

/// A real plan run: a 10 mm cube with one corner edge cut away (the shaped planner's
/// path), two candidate designs, three results wanted. Returns what the run used and the
/// layouts it found.
fn planned_rough() -> (
    RoughModel,
    PlanSettings,
    [CandidateDesign; 2],
    Vec<RoughLayout>,
) {
    let model = RoughModel::new(
        RoughBase::Block {
            x_mm: 10.0,
            y_mm: 10.0,
            z_mm: 10.0,
        },
        vec![RoughCut::Face {
            normal: [1.0, 1.0, 0.0],
            depth_mm: 1.0,
        }],
    );
    let settings = PlanSettings {
        count: 3,
        min_count: 1,
        kerf_mm: 0.3,
        allowance_mm: 0.2,
        skin_mm: 0.0,
        min_width_mm: 1.0,
        specific_gravity: 2.65,
    };
    let candidates = [
        CandidateDesign {
            entry_id: 1,
            width: 1.0,
            length: 1.5,
            height: 0.7,
            volume: 0.9,
        },
        CandidateDesign {
            entry_id: 2,
            width: 1.0,
            length: 1.0,
            height: 0.6,
            volume: 0.5,
        },
    ];
    let input = PlanInput {
        model: &model,
        settings: &settings,
        designs: &candidates,
        hulls: &[],
    };
    let planned = plan(&input, &mut |_| true).expect("the plan is not cancelled");
    assert!(
        !planned.is_empty(),
        "a 10 mm rough holds a stone of width 1"
    );
    (model, settings, candidates, planned)
}

#[test]
fn planned_layouts_survive_save_list_load_and_parse_bit_for_bit() {
    let (model, settings, candidates, planned) = planned_rough();

    // The plan's designs, as a save records them.
    let mut used: Vec<i64> = planned
        .iter()
        .flat_map(|layout| layout.stones.iter().map(|stone| stone.entry_id))
        .collect();
    used.sort_unstable();
    used.dedup();
    let records: Vec<SavedDesignDto> = used
        .iter()
        .filter_map(|id| candidates.iter().find(|c| c.entry_id == *id))
        .map(|c| {
            let shape = design_shape(&SolidExtents {
                width_caliper: c.width,
                length_caliper: c.length,
                width_axis: c.width,
                length_axis: c.length,
                height: c.height,
                volume: c.volume,
            });
            SavedDesignDto {
                entry_id: c.entry_id,
                title: format!("Design {}", c.entry_id),
                fingerprint: shape.fingerprint,
                width_caliper: shape.width_caliper,
                extents_version: shape.extents_version,
            }
        })
        .collect();
    let ranked: Vec<RankedLayout<'_>> = planned
        .iter()
        .enumerate()
        .map(|(i, layout)| RankedLayout {
            rank: i + 1,
            layout,
        })
        .collect();
    let text = serialize_checked_plan(&SerializeInput {
        name: "Planned",
        created_at: 1_790_000_000,
        library_id: Some(5),
        model: &model,
        material_name: "Quartz",
        weighed_ct: None,
        settings: &settings,
        candidate_source: CandidateSource::Filter,
        designs: &records,
        layouts: &ranked,
    })
    .expect("the planner's own output passes the loader's checks");

    let db = library();
    let id = store::save_plan(&db, "Planned", &text).expect("stored");
    let listed = store::list_saved(&db).expect("listed");
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].id, id);
    assert_eq!(listed[0].summary, plan_summary(&text));
    assert!(listed[0].summary.starts_with("Block \u{b7} Quartz \u{b7} "));

    let stored = store::load_saved(&db, id).expect("loaded");
    assert_eq!(stored.payload, text);
    assert_eq!(stored.payload_version, 1);
    let loaded = parse_and_validate_plan(&stored.payload).expect("parsed");
    assert_eq!(loaded.layouts.len(), planned.len());
    for (before, after) in planned.iter().zip(&loaded.layouts) {
        assert_eq!(bits(before), bits(after), "every float survives exactly");
        assert_eq!(before.cut_order, after.cut_order);
        assert_eq!(before.exact_fit, after.exact_fit);
        let ids = |layout: &RoughLayout| -> Vec<_> {
            layout
                .stones
                .iter()
                .map(|s| (s.entry_id, s.table_axis))
                .collect()
        };
        assert_eq!(ids(before), ids(after));
    }
}

/// The lines of `text` and of `other` that differ, as `(text's, other's)` pairs.
fn differing_lines<'a>(text: &'a str, other: &'a str) -> Vec<(&'a str, &'a str)> {
    assert_eq!(
        text.lines().count(),
        other.lines().count(),
        "same line count"
    );
    text.lines()
        .zip(other.lines())
        .filter(|(left, right)| left != right)
        .collect()
}

#[test]
fn a_rename_changes_the_table_and_the_name_line_of_the_payload_and_nothing_else() {
    let db = library();
    let text = write(&plain_block(), &[sample_layout()]);
    let id = store::save_plan(&db, "Test plan", &text).expect("stored");
    let summary_before = store::list_saved(&db).expect("listed")[0].summary.clone();

    store::rename_saved(&db, id, "Better name").expect("renamed");
    let stored = store::load_saved(&db, id).expect("loaded");
    assert_eq!(stored.name, "Better name");
    let changed = differing_lines(&text, &stored.payload);
    assert_eq!(changed.len(), 1, "{changed:?}");
    assert_eq!(changed[0].0, "name = \"Test plan\"");
    assert!(changed[0].1.starts_with("name = "), "{}", changed[0].1);
    assert_eq!(
        parse_and_validate_plan(&stored.payload)
            .expect("still a plan")
            .name,
        "Better name"
    );

    let listed = store::list_saved(&db).expect("listed");
    assert_eq!(listed[0].name, "Better name");
    assert_eq!(
        listed[0].summary, summary_before,
        "a rename keeps the summary"
    );
}

#[test]
fn renaming_or_deleting_a_plan_that_is_gone_says_so() {
    let db = library();
    let id = store::save_plan(&db, "Gone", &write(&plain_block(), &[])).expect("stored");
    store::delete_saved(&db, id).expect("deleted");
    let error = store::rename_saved(&db, id, "Again").expect_err("nothing to rename");
    assert!(error.contains("no longer exists"), "{error}");
    let error = store::delete_saved(&db, id).expect_err("nothing to delete");
    assert!(error.contains("no longer exists"), "{error}");
    let error = store::load_saved(&db, id).expect_err("nothing to load");
    assert!(error.contains("no longer exists"), "{error}");
}

#[test]
fn an_exported_plan_imports_into_another_library_unchanged_and_keeps_an_unknown_key() {
    let first = library();
    let second = library();
    let text = format!(
        "future_key = \"kept\"\n{}",
        write(&plain_block(), &[sample_layout()])
    );
    let original = parse_and_validate_plan(&text).expect("the unknown key is ignored");
    let id = store::save_plan(&first, "Test plan", &text).expect("stored");
    store::rename_saved(&first, id, "Renamed").expect("renamed");

    // Export writes the stored payload with the stored name.
    let stored = store::load_saved(&first, id).expect("loaded");
    let exported = payload_with_name(&stored.payload, &stored.name);

    // Import on the other library: parse, store, load, parse again.
    let imported = parse_and_validate_plan(&exported).expect("the export is a plan");
    let new_id = store::save_plan(&second, &imported.name, &exported).expect("stored");
    let again = store::load_saved(&second, new_id).expect("loaded");
    assert_eq!(
        parse_and_validate_plan(&again.payload).expect("parsed"),
        imported
    );
    assert!(
        again.payload.contains("future_key = \"kept\""),
        "{}",
        again.payload
    );
    let mut expected = original;
    expected.name = "Renamed".to_string();
    assert_eq!(imported, expected, "the same plan, under its new name");
}

#[test]
fn the_payload_version_is_recorded_at_save_and_compared_at_open() {
    let db = library();
    let text = write(&plain_block(), &[sample_layout()]);
    let id = store::save_plan(&db, "Current", &text).expect("stored");
    assert_eq!(
        store::load_saved(&db, id).expect("loaded").payload_version,
        1
    );
    let opened = prepare_open(&db, id).expect("opens");
    assert_eq!(opened.plan.version, 1);
    assert!(
        !opened
            .staleness
            .warnings
            .iter()
            .any(|w| w.contains("schema version")),
        "{:?}",
        opened.staleness.warnings
    );

    // A row whose record disagrees with its text is opened as its text says, with a note.
    let odd = db
        .lock()
        .expect("lock")
        .save_rough_plan("Odd", 7, &text, &plan_summary(&text), 1)
        .expect("stored");
    let opened = prepare_open(&db, odd).expect("opens");
    assert_eq!(opened.plan.version, 1);
    let note = opened
        .staleness
        .warnings
        .iter()
        .find(|w| w.contains("schema version 7"))
        .expect("a note about the record");
    assert!(note.contains("version 1"), "{note}");
}

#[test]
fn a_list_of_forty_plans_reads_no_payload() {
    let db = library();
    let text = write(&plain_block(), &[]);
    for n in 0..40 {
        store::save_plan(&db, &format!("Plan {n}"), &text).expect("stored");
    }
    let mut reads = 0;
    let rows = store::list_saved_with(&db, &mut |_| {
        reads += 1;
        Ok(None)
    })
    .expect("listed");
    assert_eq!(rows.len(), 40);
    assert_eq!(reads, 0, "the summaries are stored with the plans");
    assert_eq!(rows[0].name, "Plan 39", "newest first");
    assert_eq!(rows[0].summary, "Block \u{b7} Aquamarine \u{b7} 0 results");
}

#[test]
fn a_plan_stored_without_a_summary_is_read_once_and_then_listed_from_its_summary() {
    let db = library();
    let text = write(&plain_block(), &[sample_layout()]);
    for n in 0..3 {
        // An empty summary is what a row from before summaries were stored looks like.
        db.lock()
            .expect("lock")
            .save_rough_plan(&format!("Old {n}"), 1, &text, "", 100 + n)
            .expect("stored");
    }
    let read = |reads: &mut u32, id: i64| {
        *reads += 1;
        db.lock()
            .expect("lock")
            .get_saved_rough_plan(id)
            .map_err(|e| e.to_string())
    };

    let mut first_reads = 0;
    let first = store::list_saved_with(&db, &mut |id| read(&mut first_reads, id)).expect("listed");
    assert_eq!(first_reads, 3, "each plan is read once");
    assert!(first.iter().all(|row| row.summary == plan_summary(&text)));

    let mut second_reads = 0;
    let second =
        store::list_saved_with(&db, &mut |id| read(&mut second_reads, id)).expect("listed");
    assert_eq!(second_reads, 0, "the summaries were written back");
    assert_eq!(second, first);
}

/// Adds a design titled `title` with the extents of `width` model units (ratios fixed) and
/// returns its id.
pub(super) fn add_design(library: &Mutex<Database>, title: &str, width: f64) -> i64 {
    let db = library.lock().expect("lock");
    let count = db.all_entry_ids().expect("ids").len();
    let id = db
        .save_diagram_entry(
            &FacetingDiagramEntry {
                title: title.to_string(),
                url: format!("local://{title}-{count}.asc"),
                design_id: String::new(),
            },
            "local-import",
        )
        .expect("entry saved");
    db.save_solid_extents(
        id,
        Some(SolidExtents {
            width_caliper: width,
            length_caliper: 1.5 * width,
            width_axis: width,
            length_axis: 1.5 * width,
            height: 0.7 * width,
            volume: 0.9 * width * width * width,
        }),
        SolidExtentsSource::DesignFile,
        1,
    )
    .expect("extents saved");
    id
}

#[test]
fn a_plan_from_another_library_is_not_bound_to_the_local_designs_that_share_its_ids() {
    let db = library();
    // The plan's designs are entries 1 ("Barion Oval") and 2 ("Emerald") of the library that
    // wrote it. Here entries 1 and 2 are two other designs.
    let other_a = add_design(&db, "Zircon", 3.0);
    let other_b = add_design(&db, "Topaz", 4.0);
    assert_eq!(
        (other_a, other_b),
        (1, 2),
        "a fresh library hands out ids 1 and 2"
    );

    let text = write(&plain_block(), &[sample_layout()]);
    let plan = parse_and_validate_plan(&text).expect("a good plan");
    assert_eq!(plan.designs[0].entry_id, 1);
    let staleness = check_staleness(&db, &plan).expect("checked");
    assert_eq!(staleness.statuses[&1], DesignStatus::Deleted);
    assert_eq!(staleness.statuses[&2], DesignStatus::Deleted);

    // When the library does hold those designs under their titles, the titles find them, at
    // other ids, and the stones move there.
    let oval = add_design(&db, "Barion Oval", 1.25);
    let emerald = add_design(&db, "Emerald", 0.8);
    // Their shapes are not the plan's recorded ratios, so the match is refused ...
    let staleness = check_staleness(&db, &plan).expect("checked");
    assert_eq!(staleness.statuses[&1], DesignStatus::Deleted);
    // ... until a plan records what the library measured.
    let mut measured = plan;
    for (design, id) in measured.designs.iter_mut().zip([oval, emerald]) {
        let stored = db
            .lock()
            .expect("lock")
            .solid_extents_for(&[id])
            .expect("extents");
        let shape = design_shape(&stored[&id].extents.expect("measured"));
        design.fingerprint = shape.fingerprint;
        design.width_caliper = shape.width_caliper;
        design.extents_version = shape.extents_version;
    }
    let staleness = check_staleness(&db, &measured).expect("checked");
    assert_eq!(
        staleness.statuses[&1],
        DesignStatus::MatchedByTitle {
            resolved_entry_id: oval
        }
    );
    assert_eq!(
        staleness.statuses[&2],
        DesignStatus::MatchedByTitle {
            resolved_entry_id: emerald
        }
    );
}

#[test]
fn a_plan_keeps_its_ids_in_the_library_that_wrote_it_even_when_a_design_was_renamed() {
    let db = library();
    let renamed = add_design(&db, "Renamed since", 1.25);
    assert_eq!(renamed, 1);
    let mut plan =
        parse_and_validate_plan(&write(&plain_block(), &[sample_layout()])).expect("a good plan");
    // The plan was written by this very library: its stamp is the library's own.
    plan.library_id = db.lock().expect("lock").library_stamp().ok();
    assert!(plan.library_id.is_some());
    let shape = design_shape(&SolidExtents {
        width_caliper: 1.25,
        length_caliper: 1.875,
        width_axis: 1.25,
        length_axis: 1.875,
        height: 0.875,
        volume: 0.9 * 1.25 * 1.25 * 1.25,
    });
    plan.designs[0].fingerprint = shape.fingerprint;
    plan.designs[0].width_caliper = shape.width_caliper;
    let staleness = check_staleness(&db, &plan).expect("checked");
    assert_eq!(
        staleness.statuses[&1],
        DesignStatus::Unchanged,
        "same library: the id is the design whatever its title now is"
    );
    // Design 2 is gone from this library, and no design carries its title.
    assert_eq!(staleness.statuses[&2], DesignStatus::Deleted);
}
