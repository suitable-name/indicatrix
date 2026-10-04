//! Tests of the staleness check: the pure comparisons, and `resolve_designs` against an
//! in-memory library.

use super::*;
use indicatrix_vault::model::{
    entry::FacetingDiagramEntry,
    solid_extents::{SolidExtents, SolidExtentsSource},
};

const SAVED: [f64; 3] = [1.3912, 0.67512, 0.4123];

/// A shape with the given ratios, no width and the current rule version.
fn ratios(fingerprint: [f64; 3]) -> DesignShape {
    DesignShape {
        fingerprint,
        width_caliper: None,
        extents_version: 0,
    }
}

/// A shape with a width as well.
fn sized(fingerprint: [f64; 3], width: f64) -> DesignShape {
    DesignShape {
        width_caliper: Some(width),
        extents_version: SOLID_EXTENTS_VERSION,
        ..ratios(fingerprint)
    }
}

#[test]
fn fingerprints_match_within_a_millionth_of_the_larger_component() {
    assert!(fingerprints_match(SAVED, SAVED));
    let near = [SAVED[0] * (1.0 + 5e-7), SAVED[1], SAVED[2]];
    assert!(fingerprints_match(SAVED, near));
    let far = [SAVED[0] * (1.0 + 5e-6), SAVED[1], SAVED[2]];
    assert!(!fingerprints_match(SAVED, far));
    let last = [SAVED[0], SAVED[1], SAVED[2] * 1.001];
    assert!(!fingerprints_match(SAVED, last), "every component counts");
}

#[test]
fn an_existing_design_is_changed_only_when_a_known_fingerprint_differs() {
    let saved = ratios(SAVED);
    assert_eq!(
        status_for_existing(&saved, Some(&ratios(SAVED))),
        DesignStatus::Unchanged
    );
    assert_eq!(
        status_for_existing(&saved, Some(&ratios([1.5, 0.7, 0.4]))),
        DesignStatus::Changed
    );
    // No cached extents: cannot be compared, counts as unchanged.
    assert_eq!(status_for_existing(&saved, None), DesignStatus::Unchanged);
    // A stored all-zero fingerprint means "unknown at save time".
    assert_eq!(
        status_for_existing(&ratios([0.0; 3]), Some(&ratios([1.5, 0.7, 0.4]))),
        DesignStatus::Unchanged
    );
}

#[test]
fn a_design_drawn_twice_as_large_with_the_same_ratios_is_changed() {
    let saved = sized(SAVED, 1.0);
    // Same ratios, width 2: a uniform 2x rescale.
    assert_eq!(
        status_for_existing(&saved, Some(&sized(SAVED, 2.0))),
        DesignStatus::Changed
    );
    // Same width within a millionth is the same design.
    assert_eq!(
        status_for_existing(&saved, Some(&sized(SAVED, 1.0 + 5e-7))),
        DesignStatus::Unchanged
    );
    // An old file stored no width: only the ratios are compared, as before.
    assert_eq!(
        status_for_existing(&ratios(SAVED), Some(&sized(SAVED, 2.0))),
        DesignStatus::Unchanged
    );
}

#[test]
fn figures_of_another_measuring_rule_cannot_be_compared() {
    let mut saved = sized(SAVED, 1.0);
    saved.extents_version = SOLID_EXTENTS_VERSION + 1;
    assert!(!comparable(&saved));
    assert_eq!(
        status_for_existing(&saved, Some(&sized([2.0, 0.5, 0.3], 9.0))),
        DesignStatus::Unchanged,
        "a different rule version is not evidence of a change"
    );
    assert!(comparable(&ratios(SAVED)), "0 means the file did not say");
}

#[test]
fn adding_a_concave_tier_turns_an_unchanged_design_into_a_changed_one() {
    // The fingerprint's third ratio is V / W^3, and V is now the carved volume: the same
    // facets with a tool cut out of them have a smaller V at the same W, L and H.
    let flat = extents(0.7, 0.9, 1.0);
    let mut carved = flat;
    carved.volume *= 0.97;
    let saved = design_shape(&flat);
    assert_eq!(
        status_for_existing(&saved, Some(&design_shape(&flat))),
        DesignStatus::Unchanged
    );
    assert_eq!(
        status_for_existing(&saved, Some(&design_shape(&carved))),
        DesignStatus::Changed,
        "the carved volume moves V/W^3"
    );
    let (flat_print, carved_print) = (saved.fingerprint, design_shape(&carved).fingerprint);
    assert_eq!(
        flat_print[..2],
        carved_print[..2],
        "only the volume ratio moves"
    );
    assert!(carved_print[2] < flat_print[2]);
}

#[test]
fn a_plan_saved_under_the_convex_volume_rule_is_not_compared() {
    // A version-2 plan holds the convex V/W^3. The current rule cannot compare against
    // it, so the design is neither flagged nor trusted as measured identically.
    let flat = extents(0.7, 0.9, 1.0);
    let mut carved = flat;
    carved.volume *= 0.97;
    let mut saved = design_shape(&flat);
    saved.extents_version = 2;
    assert!(!comparable(&saved));
    assert_eq!(
        status_for_existing(&saved, Some(&design_shape(&carved))),
        DesignStatus::Unchanged
    );
}

fn candidate(entry_id: i64, shape: DesignShape) -> TitleCandidate {
    TitleCandidate { entry_id, shape }
}

#[test]
fn a_title_match_needs_exactly_one_design_with_the_stored_shape() {
    let saved = sized(SAVED, 1.0);
    let other = sized([2.0, 0.5, 0.3], 1.0);
    assert_eq!(unique_match(&[], &saved), None);
    assert_eq!(unique_match(&[candidate(7, saved)], &saved), Some(7));
    assert_eq!(
        unique_match(&[candidate(7, other), candidate(9, saved)], &saved),
        Some(9),
        "a namesake with another shape is not a match"
    );
    assert_eq!(
        unique_match(&[candidate(7, saved), candidate(9, saved)], &saved),
        None,
        "two equal matches are ambiguous"
    );
    assert_eq!(unique_match(&[candidate(7, other)], &saved), None);
    assert_eq!(
        unique_match(&[candidate(7, saved)], &ratios([0.0; 3])),
        None,
        "an unknown stored fingerprint cannot confirm a match"
    );
    assert_eq!(
        unique_match(&[candidate(7, sized(SAVED, 2.0))], &saved),
        None,
        "the same ratios at twice the size is not the stored design"
    );
}

#[test]
fn matched_designs_move_to_their_new_id_and_the_worse_status_wins_a_clash() {
    let statuses = BTreeMap::from([
        (
            1,
            DesignStatus::MatchedByTitle {
                resolved_entry_id: 50,
            },
        ),
        (2, DesignStatus::Unchanged),
        (50, DesignStatus::Changed),
        (3, DesignStatus::Deleted),
    ]);
    let remap = remap_of(&statuses);
    assert_eq!(remap, BTreeMap::from([(1, 50)]));
    let rekeyed = rekey_statuses(statuses, &remap);
    assert_eq!(rekeyed.len(), 3);
    assert_eq!(rekeyed.get(&50), Some(&DesignStatus::Changed));
    assert_eq!(rekeyed.get(&2), Some(&DesignStatus::Unchanged));
    assert_eq!(rekeyed.get(&3), Some(&DesignStatus::Deleted));
}

#[test]
fn a_chain_of_matches_moves_one_step_like_the_stones_do() {
    // 1 was found as 2, and 2 (another stored design) was found as 3. The stones move one
    // step (1 to 2, 2 to 3), so each status must sit under the id its stones now carry.
    let statuses = BTreeMap::from([
        (
            1,
            DesignStatus::MatchedByTitle {
                resolved_entry_id: 2,
            },
        ),
        (
            2,
            DesignStatus::MatchedByTitle {
                resolved_entry_id: 3,
            },
        ),
    ]);
    let remap = remap_of(&statuses);
    assert_eq!(remap, BTreeMap::from([(1, 2), (2, 3)]));
    let rekeyed = rekey_statuses(statuses, &remap);
    assert_eq!(
        rekeyed,
        BTreeMap::from([
            (
                2,
                DesignStatus::MatchedByTitle {
                    resolved_entry_id: 2
                }
            ),
            (
                3,
                DesignStatus::MatchedByTitle {
                    resolved_entry_id: 3
                }
            ),
        ])
    );
}

fn library() -> Mutex<Database> {
    Mutex::new(Database::new(Some(":memory:")).expect("an in-memory library opens"))
}

/// Extents of width 1.0 and length 1.5: fingerprint `[1.5, height, volume]`.
fn extents(height: f64, volume: f64, scale: f64) -> SolidExtents {
    SolidExtents {
        width_caliper: scale,
        length_caliper: 1.5 * scale,
        width_axis: scale,
        length_axis: 1.5 * scale,
        height: height * scale,
        volume: volume * scale * scale * scale,
    }
}

/// The catalogue shape the tests call "the design": ratios `[1.5, 0.7, 0.9]` at width 1.
fn design_extents() -> SolidExtents {
    extents(0.7, 0.9, 1.0)
}

/// Adds a design titled `title` (with `measured` extents if given) and returns its id.
fn add(library: &Mutex<Database>, title: &str, measured: Option<SolidExtents>) -> i64 {
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
    if let Some(found) = measured {
        db.save_solid_extents(id, Some(found), SolidExtentsSource::DesignFile, 1)
            .expect("extents saved");
    }
    id
}

/// The record a plan stores for `entry_id` when the design measured `found`.
fn record(entry_id: i64, title: &str, found: &SolidExtents) -> SavedDesignDto {
    let shape = design_shape(found);
    SavedDesignDto {
        entry_id,
        title: title.to_string(),
        fingerprint: shape.fingerprint,
        width_caliper: shape.width_caliper,
        extents_version: shape.extents_version,
    }
}

fn status_of(staleness: &Staleness, entry_id: i64) -> DesignStatus {
    *staleness.statuses.get(&entry_id).expect("a status")
}

#[test]
fn a_design_that_is_present_and_unchanged_needs_no_warning() {
    let library = library();
    let id = add(&library, "Barion Oval", Some(design_extents()));
    let designs = [record(id, "Barion Oval", &design_extents())];
    let result = resolve_designs(&library, &designs, None).expect("checked");
    assert_eq!(status_of(&result, id), DesignStatus::Unchanged);
    assert!(result.warnings.is_empty(), "{:?}", result.warnings);
}

#[test]
fn a_design_redrawn_twice_as_large_is_changed_although_its_ratios_are_the_same() {
    let library = library();
    let id = add(&library, "Barion Oval", Some(extents(0.7, 0.9, 2.0)));
    let designs = [record(id, "Barion Oval", &design_extents())];
    let result = resolve_designs(&library, &designs, None).expect("checked");
    assert_eq!(status_of(&result, id), DesignStatus::Changed);
}

#[test]
fn a_design_with_other_proportions_is_changed() {
    let library = library();
    let id = add(&library, "Barion Oval", Some(extents(0.8, 0.9, 1.0)));
    let designs = [record(id, "Barion Oval", &design_extents())];
    let result = resolve_designs(&library, &designs, None).expect("checked");
    assert_eq!(status_of(&result, id), DesignStatus::Changed);
}

#[test]
fn a_design_without_extents_is_unchanged_but_named_in_a_warning() {
    let library = library();
    let unmeasured = add(&library, "Never measured", None);
    let measured = add(&library, "Measured", Some(design_extents()));
    let designs = [
        record(unmeasured, "Never measured", &design_extents()),
        record(measured, "Measured", &design_extents()),
    ];
    let result = resolve_designs(&library, &designs, None).expect("checked");
    assert_eq!(status_of(&result, unmeasured), DesignStatus::Unchanged);
    assert_eq!(status_of(&result, measured), DesignStatus::Unchanged);
    assert_eq!(result.warnings.len(), 1, "{:?}", result.warnings);
    let warning = &result.warnings[0];
    assert!(warning.contains("\"Never measured\""), "{warning}");
    assert!(!warning.contains("\"Measured\""), "{warning}");
    assert!(warning.contains("re-measure"), "{warning}");
}

#[test]
fn a_deleted_id_is_found_again_under_its_title_when_one_design_has_the_same_shape() {
    let library = library();
    let new_id = add(&library, "  barion OVAL ", Some(design_extents()));
    add(&library, "Barion Oval Wide", Some(design_extents()));
    let designs = [record(9_999, "Barion Oval", &design_extents())];
    let result = resolve_designs(&library, &designs, None).expect("checked");
    assert_eq!(
        status_of(&result, 9_999),
        DesignStatus::MatchedByTitle {
            resolved_entry_id: new_id
        }
    );
}

#[test]
fn two_namesakes_of_the_same_shape_are_ambiguous_and_the_design_stays_deleted() {
    let library = library();
    add(&library, "Barion Oval", Some(design_extents()));
    add(&library, "Barion Oval", Some(design_extents()));
    let designs = [record(9_999, "Barion Oval", &design_extents())];
    let result = resolve_designs(&library, &designs, None).expect("checked");
    assert_eq!(status_of(&result, 9_999), DesignStatus::Deleted);
}

#[test]
fn a_namesake_of_another_shape_makes_a_deleted_id_changed_and_no_namesake_makes_it_deleted() {
    let library = library();
    add(&library, "Barion Oval", Some(extents(0.8, 0.9, 1.0)));
    let designs = [
        record(9_998, "Barion Oval", &design_extents()),
        record(9_999, "Nobody has this title", &design_extents()),
    ];
    let result = resolve_designs(&library, &designs, None).expect("checked");
    assert_eq!(status_of(&result, 9_998), DesignStatus::Changed);
    assert_eq!(status_of(&result, 9_999), DesignStatus::Deleted);
}

#[test]
fn an_ignored_namesake_still_counts() {
    let library = library();
    let hidden = add(&library, "Barion Oval", Some(design_extents()));
    library
        .lock()
        .expect("lock")
        .set_diagram_ignored(hidden, true)
        .expect("ignored");
    let designs = [record(9_999, "Barion Oval", &design_extents())];
    let result = resolve_designs(&library, &designs, None).expect("checked");
    assert_eq!(
        status_of(&result, 9_999),
        DesignStatus::MatchedByTitle {
            resolved_entry_id: hidden
        }
    );
}

/// A library whose title search fails.
struct NoTitleSearch(Mutex<Database>);

impl LibraryView for NoTitleSearch {
    fn titles_of(&self, ids: &[i64]) -> Result<BTreeMap<i64, String>, String> {
        self.0.titles_of(ids)
    }
    fn extents_of(&self, ids: &[i64]) -> Result<BTreeMap<i64, StoredSolidExtents>, String> {
        self.0.extents_of(ids)
    }
    fn ids_titled(&self, _titles: &[String]) -> Result<BTreeMap<String, Vec<i64>>, String> {
        Err("disk is gone".to_string())
    }
    fn stamp(&self) -> Option<u32> {
        self.0.stamp()
    }
}

#[test]
fn a_failing_title_search_warns_and_leaves_the_designs_deleted() {
    let library = NoTitleSearch(library());
    add(&library.0, "Barion Oval", Some(design_extents()));
    let present = add(&library.0, "Present", Some(design_extents()));
    let designs = [
        record(9_999, "Barion Oval", &design_extents()),
        record(present, "Present", &design_extents()),
    ];
    let result = resolve_designs(&library, &designs, None).expect("the check goes on");
    assert_eq!(status_of(&result, 9_999), DesignStatus::Deleted);
    assert_eq!(status_of(&result, present), DesignStatus::Unchanged);
    assert_eq!(result.warnings.len(), 1, "{:?}", result.warnings);
    assert!(result.warnings[0].contains("disk is gone"));
    assert!(
        result.warnings[0].contains("1 design:"),
        "{}",
        result.warnings[0]
    );
}

/// A library with a stamp, and a design `id` that a foreign plan thinks is "Mine".
struct Collision {
    library: Mutex<Database>,
    local: i64,
    stamp: u32,
}

fn collision(local_extents: Option<SolidExtents>) -> Collision {
    let library = library();
    let local = add(&library, "Someone else's design", local_extents);
    let stamp = library.stamp().expect("the library has a stamp");
    Collision {
        library,
        local,
        stamp,
    }
}

#[test]
fn a_colliding_id_with_another_title_and_shape_is_not_the_stored_design() {
    let case = collision(Some(extents(0.8, 0.5, 1.0)));
    let designs = [record(case.local, "Mine", &design_extents())];
    // A plan from another library (a different stamp) and one with no stamp at all.
    for plan_library in [Some(case.stamp + 1), None] {
        let result = resolve_designs(&case.library, &designs, plan_library).expect("checked");
        assert_eq!(
            status_of(&result, case.local),
            DesignStatus::Deleted,
            "{plan_library:?}"
        );
    }
}

#[test]
fn a_colliding_id_is_not_trusted_when_the_stored_fingerprint_is_unknown() {
    let case = collision(Some(design_extents()));
    let mut unknown = record(case.local, "Mine", &design_extents());
    unknown.fingerprint = [0.0; 3];
    unknown.width_caliper = None;
    unknown.extents_version = 0;
    let result = resolve_designs(&case.library, &[unknown], None).expect("checked");
    assert_eq!(
        status_of(&result, case.local),
        DesignStatus::Deleted,
        "with nothing to compare, a different title decides"
    );
}

#[test]
fn a_colliding_id_without_cached_extents_is_not_trusted() {
    let case = collision(None);
    let designs = [record(case.local, "Mine", &design_extents())];
    let result = resolve_designs(&case.library, &designs, None).expect("checked");
    assert_eq!(status_of(&result, case.local), DesignStatus::Deleted);
}

#[test]
fn a_colliding_id_gives_way_to_the_design_with_the_stored_title() {
    let case = collision(Some(extents(0.8, 0.5, 1.0)));
    let mine = add(&case.library, "Mine", Some(design_extents()));
    let designs = [record(case.local, "Mine", &design_extents())];
    let result = resolve_designs(&case.library, &designs, None).expect("checked");
    assert_eq!(
        status_of(&result, case.local),
        DesignStatus::MatchedByTitle {
            resolved_entry_id: mine
        }
    );
}

#[test]
fn a_colliding_id_whose_design_has_the_stored_shape_is_a_renamed_design() {
    let case = collision(Some(design_extents()));
    let designs = [record(case.local, "Mine", &design_extents())];
    let result = resolve_designs(&case.library, &designs, None).expect("checked");
    assert_eq!(status_of(&result, case.local), DesignStatus::Unchanged);
}

#[test]
fn in_the_same_library_a_renamed_design_keeps_its_id_whatever_its_new_title() {
    let case = collision(Some(extents(0.8, 0.5, 1.0)));
    let designs = [record(case.local, "Mine", &design_extents())];
    let result = resolve_designs(&case.library, &designs, Some(case.stamp)).expect("checked");
    // The id-first path: same library, so the design at the id is the stored design. Its
    // shape is what differs here, and that is reported as a change.
    assert_eq!(status_of(&result, case.local), DesignStatus::Changed);

    let same_shape = collision(Some(design_extents()));
    let designs = [record(same_shape.local, "Mine", &design_extents())];
    let result =
        resolve_designs(&same_shape.library, &designs, Some(same_shape.stamp)).expect("checked");
    assert_eq!(
        status_of(&result, same_shape.local),
        DesignStatus::Unchanged
    );
}

#[test]
fn the_title_a_save_invents_does_not_count_as_a_different_title() {
    let case = collision(Some(design_extents()));
    let fallback = format!("Design {}", case.local);
    let designs = [record(case.local, &fallback, &design_extents())];
    let result = resolve_designs(&case.library, &designs, Some(case.stamp + 1)).expect("checked");
    assert_eq!(status_of(&result, case.local), DesignStatus::Unchanged);
}

#[test]
fn designs_measured_by_another_rule_version_are_named_in_a_warning_and_not_flagged() {
    let library = library();
    let id = add(&library, "Old rule", Some(extents(0.8, 0.5, 1.0)));
    let mut stored = record(id, "Old rule", &design_extents());
    stored.extents_version = SOLID_EXTENTS_VERSION + 1;
    let result = resolve_designs(&library, &[stored], None).expect("checked");
    assert_eq!(status_of(&result, id), DesignStatus::Unchanged);
    assert_eq!(result.warnings.len(), 1, "{:?}", result.warnings);
    assert!(result.warnings[0].contains("\"Old rule\""));
    assert!(result.warnings[0].contains("earlier version"));
}

#[test]
fn a_long_list_of_unmeasured_designs_is_cut_short_in_the_warning() {
    let library = library();
    let designs: Vec<SavedDesignDto> = (0..8)
        .map(|n| {
            let title = format!("Unmeasured {n}");
            let id = add(&library, &title, None);
            record(id, &title, &design_extents())
        })
        .collect();
    let result = resolve_designs(&library, &designs, None).expect("checked");
    let warning = &result.warnings[0];
    assert!(
        warning.starts_with("8 designs have not been re-measured"),
        "{warning}"
    );
    assert!(warning.contains("\"Unmeasured 4\""), "{warning}");
    assert!(!warning.contains("\"Unmeasured 5\""), "{warning}");
    assert!(warning.contains("and 3 more"), "{warning}");
}
