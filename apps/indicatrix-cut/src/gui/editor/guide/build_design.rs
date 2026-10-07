//! "Build this design" on the desktop: reads a library design, has the editor crate turn it
//! into a step-by-step rebuild lesson off the UI thread, registers the lesson with the
//! guide catalogue and starts it.
//!
//! The lesson itself is `indicatrix_editor::guide::build_this_design_guide` (pure, tested
//! there). This module is the shell around it:
//!
//! - **Reading the design.** A local library entry is read straight from the database the
//!   way Load Selected reads it (`loading::design_from_full_record`); a remote one is
//!   fetched as `.asc` text (`fetch_remote_design_source`). Nothing is opened in the
//!   editor: the lesson holds the original in the goal of its last step, and the learner
//!   builds in a NEW design, which the New Design action (with its unsaved-changes
//!   question) makes in the lesson's first step.
//! - **Generating off the UI thread.** The generator may solve the design (a large one
//!   takes seconds), so it runs on a worker thread and the result comes back through
//!   `upgrade_in_event_loop`. `BuildDesignModel.busy` is set meanwhile, so a second click
//!   cannot start a second run.
//! - **Completion.** The lesson's id is `build:<design key>`; the guide's `finished`
//!   callback records that id like any other, so the tutorial browser marks it done under
//!   "Built from the library".
//! - **The compare step.** On entering it, the original is held as the snapshot "Compare"
//!   and "Compare visually..." measure against ([`step_entered`]).

use super::{launch, register_generated_guide, runtime};
use crate::{
    BuildDesignModel, GuideModel, MainWindow,
    bridge::library::source::LibrarySource,
    gui::{
        editor::{callbacks::hold_reference_snapshot, loading},
        library::remote::fetch_remote_design_source,
        show_toast,
    },
};
use indicatrix::geometry::meet_solver::{SolveStrategy, SolvedTier};
use indicatrix_cut_core::Design;
use indicatrix_editor::guide::{
    BuildGuideError, Guide, build_this_design_guide, is_compare_step, original_label,
    reference_design,
};
use indicatrix_vault::{
    db::sqlite::Database,
    model::design_key::{catalogue_design_uuid, normalize_design_uuid},
};
use slint::ComponentHandle;
use std::{
    panic::{AssertUnwindSafe, catch_unwind},
    path::Path,
    sync::{Arc, Mutex, PoisonError},
};

/// Why a design that only has an angle table cannot be turned into a lesson.
const PLACEHOLDER_REFUSAL: &str = "This library design only has an angle table, not cutting \
     instructions with depths, so a lesson cannot be built from it.";

/// What a lesson is made from.
#[derive(Debug)]
struct Target {
    /// The library design, as the editor would open it.
    design: Design,
    /// What the lesson calls the design.
    title: String,
    /// The library key the lesson's id and completion mark are made from.
    key: String,
}

/// The original a lesson holds while its compare step is open, ready to be held as the
/// editor's snapshot.
struct HeldReference {
    design: Design,
    solved: Vec<SolvedTier>,
    label: String,
}

/// Wires `BuildDesignModel.start`. `BuildDesignModel.start_selected` is answered in Slint
/// (it forwards the selected library entry's id to `start`).
pub(super) fn setup(
    ui: &MainWindow,
    db: &Arc<Mutex<Database>>,
    source: &Arc<Mutex<LibrarySource>>,
) {
    let db = Arc::clone(db);
    let source = Arc::clone(source);
    let ui_weak = ui.as_weak();
    ui.global::<BuildDesignModel>().on_start(move |entry_id| {
        if let Some(ui) = ui_weak.upgrade() {
            start(&ui, &db, &source, entry_id);
        }
    });
}

/// `BuildDesignModel.start`: reads library entry `entry_id` and starts its lesson.
fn start(
    ui: &MainWindow,
    db: &Arc<Mutex<Database>>,
    source: &Arc<Mutex<LibrarySource>>,
    entry_id: i32,
) {
    let model = ui.global::<BuildDesignModel>();
    if model.get_busy() {
        show_toast(ui, "A lesson is already being prepared.", "info");
        return;
    }
    if entry_id < 0 {
        show_toast(ui, "Select a library design first.", "info");
        return;
    }
    let current_source = source
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone();
    if let Some(worker) = current_source.worker().cloned() {
        model.set_busy(true);
        let entry_id = i64::from(entry_id);
        fetch_remote_design_source(ui.as_weak(), worker, entry_id, move |ui, result| {
            let target = result
                .and_then(|remote| remote_target(entry_id, &remote.file_name, &remote.asc_text));
            match target {
                Ok(target) => generate(ui, target),
                Err(message) => fail(ui, &message),
            }
        });
        return;
    }
    match local_target(db, entry_id) {
        Ok(target) => {
            model.set_busy(true);
            generate(ui, target);
        }
        Err(message) => show_toast(ui, &message, "error"),
    }
}

/// The design of local library entry `entry_id`, read the way Load Selected reads it.
fn local_target(db: &Arc<Mutex<Database>>, entry_id: i32) -> Result<Target, String> {
    let full = db
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get_diagram_full(i64::from(entry_id));
    let Ok(Some(full)) = full else {
        return Err("Diagram detail not found.".to_owned());
    };
    let loaded = loading::design_from_full_record(&full)?;
    if loaded.used_placeholder {
        return Err(PLACEHOLDER_REFUSAL.to_owned());
    }
    Ok(Target {
        key: lesson_key(&loaded.metadata.id, &full.url, full.entry_id),
        title: full.title.clone(),
        design: loaded.design,
    })
}

/// The design of remote library entry `entry_id`, whose `.asc` file `file_name` has the text
/// `asc_text`.
fn remote_target(entry_id: i64, file_name: &str, asc_text: &str) -> Result<Target, String> {
    let loaded = loading::design_from_asc_text(file_name, asc_text, None).map_err(|error| {
        format!("'{file_name}' failed to parse as a .asc cutting instructions: {error}")
    })?;
    Ok(Target {
        key: remote_lesson_key(entry_id, file_name),
        title: title_from_file_name(file_name),
        design: loaded.design,
    })
}

/// The library key a local entry's lesson is recorded under: the design's own id when its
/// file has a valid one, else the catalogue entry's name-based UUID -- the same id the
/// editor gives the design when it opens it, so one design has one key everywhere.
fn lesson_key(stored_id: &str, url: &str, entry_id: i64) -> String {
    if let Some(id) = normalize_design_uuid(stored_id) {
        return id;
    }
    if url.trim().is_empty() {
        catalogue_design_uuid(&format!("local://entry/{entry_id}"))
    } else {
        catalogue_design_uuid(url)
    }
}

/// The key of a remote entry's lesson. A remote entry has no address of its own on this
/// side, so the key is made from the entry's number on its library and its file's name.
fn remote_lesson_key(entry_id: i64, file_name: &str) -> String {
    catalogue_design_uuid(&format!("remote://library/{entry_id}/{file_name}"))
}

/// A design's name from the name of its file: no folders, no extension.
fn title_from_file_name(file_name: &str) -> String {
    Path::new(file_name)
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or(file_name)
        .trim()
        .to_owned()
}

/// Builds the lesson on a worker thread and finishes on the UI thread.
fn generate(ui: &MainWindow, target: Target) {
    show_toast(
        ui,
        &format!(
            "Preparing the lesson for '{}'. A large design can take a few seconds.",
            target.title
        ),
        "info",
    );
    let ui_weak = ui.as_weak();
    let spawned = std::thread::Builder::new()
        .name("build-lesson".to_owned())
        .spawn(move || {
            let outcome = build(&target);
            let _ = ui_weak.upgrade_in_event_loop(move |ui| finish(&ui, outcome));
        });
    if spawned.is_err() {
        fail(
            ui,
            "The lesson could not be started: no thread was available.",
        );
    }
}

/// The lesson for `target`. A panic in the generator becomes an error, so the busy flag is
/// always cleared.
fn build(target: &Target) -> Result<Guide, BuildGuideError> {
    catch_unwind(AssertUnwindSafe(|| {
        build_this_design_guide(&target.design, None, &target.title, &target.key)
    }))
    .unwrap_or_else(|_| {
        Err(BuildGuideError::NotFit(
            "the lesson generator stopped unexpectedly.".to_owned(),
        ))
    })
}

/// Clears the busy flag and says why no lesson was started.
fn fail(ui: &MainWindow, message: &str) {
    ui.global::<BuildDesignModel>().set_busy(false);
    show_toast(ui, message, "error");
}

/// Registers the finished lesson and starts it, or says why there is none.
fn finish(ui: &MainWindow, outcome: Result<Guide, BuildGuideError>) {
    ui.global::<BuildDesignModel>().set_busy(false);
    match outcome {
        Ok(guide) => {
            let id = guide.id.clone();
            match register_generated_guide(guide) {
                Ok(()) => launch::start_guide(ui, &id),
                Err(reason) => show_toast(
                    ui,
                    &format!("The lesson could not be added: {reason}"),
                    "error",
                ),
            }
        }
        Err(error) => show_toast(ui, &error.to_string(), "error"),
    }
}

/// The original `guide` holds while its step `index` is open, ready to be held as the
/// snapshot, or `None` when that step is not a "Build this design" lesson's compare step.
/// Every tier of the original is pinned to the depth the lesson states, so the compare
/// table reads the depths the learner is asked to reach.
fn reference_to_hold(guide: &Guide, index: usize) -> Option<HeldReference> {
    if !is_compare_step(guide, index) {
        return None;
    }
    let original = reference_design(guide)?;
    Some(HeldReference {
        design: original.design.clone(),
        solved: original
            .masts
            .iter()
            .map(|&mast| SolvedTier {
                mast,
                strategy: SolveStrategy::ScaleReference,
                detail: "stated by the lesson".to_owned(),
            })
            .collect(),
        label: original_label(guide),
    })
}

/// Called when a guide step has been entered. Entering the compare step of a "Build this
/// design" lesson holds the original design as the snapshot, so Compare (on the command bar)
/// and Compare visually... line it up against the learner's stone. This replaces a snapshot
/// the learner took earlier in the session.
pub(super) fn step_entered(ui: &MainWindow) {
    let model = ui.global::<GuideModel>();
    let Ok(index) = usize::try_from(model.get_step_index()) else {
        return;
    };
    let Some(guide) = runtime::guide(model.get_guide_id().as_str()) else {
        return;
    };
    if let Some(held) = reference_to_hold(&guide, index) {
        hold_reference_snapshot(ui, held.design, held.solved, &held.label);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use indicatrix_cut_core::{ConstraintTier, PreformSpec, ScheduleMeta};
    use indicatrix_editor::guide::{
        COMPARE_STEP_TITLE, build_guide_id, is_valid_guide_id, worked_example_guide,
    };

    const KEY: &str = "5f0e7a52-1c3d-4c1e-9a55-0d6f3c2b7e11";

    fn round_brilliant() -> Design {
        Design::new(
            PreformSpec::cylinder(96, 1.5, 1.0, 1.5),
            ScheduleMeta::standard_round_brilliant(),
            ConstraintTier::standard_round_brilliant(),
        )
    }

    fn lesson() -> Guide {
        build(&Target {
            design: round_brilliant(),
            title: "Standard Round Brilliant".to_owned(),
            key: KEY.to_owned(),
        })
        .expect("a lesson for the round brilliant")
    }

    #[test]
    fn the_target_is_built_into_a_lesson_under_its_key() {
        let guide = lesson();
        assert_eq!(guide.id, build_guide_id(KEY));
        assert_eq!(guide.title, "Build Standard Round Brilliant");
        assert!(guide.problem().is_none());
    }

    #[test]
    fn a_design_without_tiers_is_refused_with_the_generators_sentence() {
        let empty = Design::new(
            PreformSpec::cylinder(96, 1.5, 1.0, 1.5),
            ScheduleMeta::standard_round_brilliant(),
            Vec::new(),
        );
        let error = build(&Target {
            design: empty,
            title: "Nothing".to_owned(),
            key: KEY.to_owned(),
        })
        .expect_err("nothing to rebuild");
        assert_eq!(error, BuildGuideError::NoTiers);
    }

    #[test]
    fn only_the_compare_step_holds_the_original() {
        let guide = lesson();
        let compare = guide
            .steps
            .iter()
            .position(|step| step.title == COMPARE_STEP_TITLE)
            .expect("a compare step");
        for index in 0..guide.steps.len() {
            assert_eq!(
                reference_to_hold(&guide, index).is_some(),
                index == compare,
                "step {index}"
            );
        }
        assert!(reference_to_hold(&guide, guide.steps.len() + 3).is_none());
        assert!(reference_to_hold(&worked_example_guide(), 3).is_none());
    }

    #[test]
    fn the_held_original_has_one_solved_depth_per_tier_and_a_label() {
        let guide = lesson();
        let compare = guide
            .steps
            .iter()
            .position(|step| step.title == COMPARE_STEP_TITLE)
            .expect("a compare step");
        let held = reference_to_hold(&guide, compare).expect("held at the compare step");
        assert_eq!(held.design.tiers.len(), 8);
        assert_eq!(held.solved.len(), held.design.tiers.len());
        assert_eq!(held.label, "Original: Standard Round Brilliant");
        // The round brilliant's girdle is the tier pinned at 1.0.
        let girdle = held
            .design
            .tiers
            .iter()
            .position(|tier| tier.name == "Girdle")
            .expect("a girdle");
        assert_eq!(held.solved[girdle].mast, 1.0);
        assert!(
            held.solved
                .iter()
                .all(|tier| matches!(tier.strategy, SolveStrategy::ScaleReference))
        );
    }

    #[test]
    fn a_files_own_id_is_the_lessons_key() {
        let key = lesson_key(&KEY.to_uppercase(), "local://a.asc", 7);
        assert_eq!(key, KEY, "the stored id, in the database's lower case");
    }

    #[test]
    fn a_file_without_an_id_is_keyed_by_its_catalogue_address() {
        let first = lesson_key("", "local://round-brilliant.asc", 7);
        assert_eq!(first, catalogue_design_uuid("local://round-brilliant.asc"));
        assert_eq!(
            first,
            lesson_key("not a uuid", "local://round-brilliant.asc", 9)
        );
        assert_ne!(first, lesson_key("", "local://other.asc", 7));
    }

    #[test]
    fn an_entry_without_an_address_is_keyed_by_its_number() {
        let first = lesson_key("", "  ", 7);
        assert_eq!(first, lesson_key("", "", 7));
        assert_ne!(first, lesson_key("", "", 8));
    }

    #[test]
    fn every_key_makes_a_valid_lesson_id() {
        for key in [
            lesson_key(KEY, "", 1),
            lesson_key("", "https://example.org/a b.asc", 1),
            lesson_key("", "", 1),
            remote_lesson_key(12, "round brilliant.asc"),
        ] {
            assert!(is_valid_guide_id(&build_guide_id(&key)), "{key}");
        }
    }

    #[test]
    fn a_remote_entry_is_keyed_by_its_number_and_file() {
        let first = remote_lesson_key(12, "round-brilliant.asc");
        assert_eq!(first, remote_lesson_key(12, "round-brilliant.asc"));
        assert_ne!(first, remote_lesson_key(13, "round-brilliant.asc"));
        assert_ne!(first, remote_lesson_key(12, "emerald.asc"));
    }

    #[test]
    fn a_title_comes_from_the_file_name() {
        assert_eq!(
            title_from_file_name("round-brilliant.asc"),
            "round-brilliant"
        );
        assert_eq!(
            title_from_file_name("designs/emerald cut.asc"),
            "emerald cut"
        );
        assert_eq!(title_from_file_name("  plain  "), "plain");
        assert_eq!(title_from_file_name(""), "");
    }

    #[test]
    fn a_remote_file_that_does_not_parse_says_which() {
        let error = remote_target(3, "broken.asc", "this is not a schedule")
            .expect_err("unparseable text is refused");
        assert!(error.contains("'broken.asc'"), "{error}");
    }

    #[test]
    fn the_refusal_for_an_angle_table_is_a_plain_sentence() {
        assert!(PLACEHOLDER_REFUSAL.is_ascii());
        assert!(PLACEHOLDER_REFUSAL.contains("angle table"));
        assert!(!PLACEHOLDER_REFUSAL.contains("  "));
    }
}
