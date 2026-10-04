//! The desktop's concave-tier path, end to end through public APIs: a tier typed into the
//! concave form is parsed, turned into an edit and applied to an `EditorSession` exactly
//! as `gui::editor::callbacks::tier_actions::concave_tier` does; the design is written to
//! and read back from the native file; and the result is checked where a cutter sees it,
//! the cutting sheet and the solid preview's pick buffer.
//!
//! The solid frame's FNV is pinned by agreement between independent paths (the session's
//! design and the one reloaded from the file draw the same bytes, and the tools change the
//! picture). An absolute golden value would hold the platform math library's bits, so, as
//! in `indicatrix-solid`'s own concave pins, one is only compared on Windows and is `None`
//! until recorded there.

use indicatrix_cut::gui::solid_preview::mesh_cache::fnv1a_64;
use indicatrix_cut_core::{
    Design, ManufacturabilityWarning,
    design::ConcaveTier,
    native::{DesignExtras, design_from_str, design_to_string},
};
use indicatrix_editor::{
    EditorSession,
    loading::{concave_tier_form_fields, parse_concave_tier_form},
    tier_save::concave_tier_save_edit,
    view_model::solid_status::design_to_gpu_geometry,
};
use indicatrix_solid::preview::{CameraPose, PreviewPipeline, RedrawRequest, StoneGeometryBuf};

/// Golden FNV of the concave fixture's solid pick buffer for [`camera`], recorded on
/// Windows only (see this file's module doc comment).
const PICK_PIN: Option<u64> = None;

/// The fixture's flat half in a fresh session, and its concave tiers on their own.
fn flat_session_and_concave_tiers() -> (EditorSession, Vec<ConcaveTier>) {
    let mut flat = Design::concave_fixture();
    let concave = std::mem::take(&mut flat.concave_tiers);
    let mut session = EditorSession::fresh();
    session.design = flat;
    (session, concave)
}

/// Adds `tier` the way the concave form's Save does: fields -> parse -> save edit -> apply.
fn save_through_the_form(session: &mut EditorSession, index: Option<usize>, tier: &ConcaveTier) {
    let fields = concave_tier_form_fields(tier);
    let parsed = parse_concave_tier_form(&fields, session.design.meta.gear_teeth)
        .expect("the form accepts what it just read back");
    let edit = concave_tier_save_edit(&session.design, index, parsed);
    session.apply(edit).expect("the save edit applies");
}

/// The session the callbacks would hold after authoring the whole fixture by hand.
fn authored_session() -> EditorSession {
    let (mut session, concave) = flat_session_and_concave_tiers();
    for tier in &concave {
        save_through_the_form(&mut session, None, tier);
    }
    session
}

const fn camera() -> CameraPose {
    CameraPose {
        yaw: 0.6,
        pitch: 1.0,
        distance: 3.0,
    }
}

/// The solid preview's pick buffer and image for `design` at [`camera`], and how many tools
/// the frame drew.
fn solid_frame(design: &Design) -> (u64, u64, usize) {
    let (planes, tools, placements) = design_to_gpu_geometry(design);
    let mut pipeline = PreviewPipeline::new();
    let frame = pipeline
        .render(RedrawRequest::Reproject {
            geometry: StoneGeometryBuf {
                planes,
                tools,
                placements,
            },
            camera: camera(),
            size: (160, 120),
            view_mode: 0,
            gear: Some((design.meta.gear_teeth_abs(), 0.0)),
        })
        .expect("a reproject always resolves");
    let pick: Vec<u8> = frame
        .pick
        .pick
        .iter()
        .flat_map(|id| id.to_le_bytes())
        .collect();
    (
        fnv1a_64(pick.iter().copied()),
        fnv1a_64(pipeline.solid_rgba().iter().copied()),
        frame.stone.tools.len(),
    )
}

#[test]
fn authoring_the_fixture_through_the_concave_form_reproduces_the_fixture() {
    let session = authored_session();
    assert_eq!(
        session.design.concave_tiers,
        Design::concave_fixture().concave_tiers,
        "form -> parse -> concave_tier_save_edit -> apply must lose nothing"
    );
    assert_eq!(session.design.tiers, Design::concave_fixture().tiers);
}

#[test]
fn editing_a_concave_tier_through_the_form_is_one_undoable_step() {
    let mut session = authored_session();
    let original = session.design.concave_tiers.clone();
    let mut wider = original[0].clone();
    wider.diameter_ratio = 0.3;
    save_through_the_form(&mut session, Some(0), &wider);
    assert!((session.design.concave_tiers[0].diameter_ratio - 0.3).abs() < 1e-12);
    assert_eq!(
        session.design.concave_tiers[1], original[1],
        "the other tier is untouched"
    );
    session.undo().expect("undo replays").expect("one step");
    assert_eq!(session.design.concave_tiers, original);
}

#[test]
fn the_native_file_round_trips_concave_tiers_and_their_cutting_sheet_text() {
    let session = authored_session();
    let text = design_to_string(&session.design, None, &DesignExtras::default())
        .expect("the design writes as a native file");
    let loaded = design_from_str(&text)
        .expect("the native file reads back")
        .design;
    assert_eq!(loaded.concave_tiers, session.design.concave_tiers);
    assert_eq!(loaded.tiers, session.design.tiers);

    let sheet = |design: &Design| {
        let solved = design.solve().expect("the fixture solves");
        design.cutting_sheet(&solved).to_text()
    };
    let sheet_text = sheet(&loaded);
    assert_eq!(
        sheet_text,
        sheet(&session.design),
        "the reloaded design prints the same sheet"
    );
    assert_eq!(
        sheet_text,
        sheet(&Design::concave_fixture()),
        "and the sheet the fixture itself prints"
    );

    // Each concave tier prints as two lines: the facet line, then its tool line directly
    // under it, built from the very fields `ConcaveTier::second_line_fields` gives every
    // other output.
    let lines: Vec<&str> = sheet_text.lines().collect();
    for tier in &loaded.concave_tiers {
        let [code, _theta, displacement, details] = tier.second_line_fields();
        let facet_line = lines
            .iter()
            .position(|line| line.contains(&tier.name))
            .unwrap_or_else(|| panic!("the sheet lists {}", tier.name));
        let tool_line = lines
            .get(facet_line + 1)
            .unwrap_or_else(|| panic!("{} has a tool line", tier.name));
        assert!(tool_line.trim_start().starts_with(&code), "{tool_line:?}");
        assert!(tool_line.contains(&displacement), "{tool_line:?}");
        assert!(tool_line.contains(&details), "{tool_line:?}");
    }
}

#[test]
fn the_solid_pick_buffer_is_the_same_before_and_after_the_native_file_and_shows_the_tools() {
    let session = authored_session();
    let text = design_to_string(&session.design, None, &DesignExtras::default())
        .expect("the design writes as a native file");
    let loaded = design_from_str(&text)
        .expect("the native file reads back")
        .design;

    let (pick, image, tool_count) = solid_frame(&session.design);
    // A concave tier resolves to one tool primitive per index (placement), so the frame
    // draws the sum of the tiers' index counts, each placement naming one of the tiers.
    let expected_tools: usize = session
        .design
        .concave_tiers
        .iter()
        .map(|tier| tier.indices.len())
        .sum();
    assert_eq!(tool_count, expected_tools, "one tool per concave index");
    let (_, _, placements) = design_to_gpu_geometry(&session.design);
    assert_eq!(placements.len(), expected_tools);
    let tiers_hit: std::collections::BTreeSet<usize> =
        placements.iter().map(|&(tier, _)| tier).collect();
    assert_eq!(
        tiers_hit,
        (0..session.design.concave_tiers.len()).collect(),
        "every concave tier is drawn, and every placement maps back to one of them"
    );
    assert_eq!(
        (pick, image, tool_count),
        solid_frame(&loaded),
        "a reloaded design draws the stone the authored one drew"
    );

    // Dropping the tools changes both buffers: they are in the picture.
    let mut flat = session.design;
    flat.concave_tiers.clear();
    let (flat_pick, flat_image, flat_tools) = solid_frame(&flat);
    assert_eq!(flat_tools, 0);
    assert_ne!(
        flat_pick, pick,
        "the tool facets own pixels of the pick buffer"
    );
    assert_ne!(flat_image, image);

    if cfg!(windows)
        && let Some(pinned) = PICK_PIN
    {
        assert_eq!(pick, pinned);
    }
}

#[test]
fn a_file_export_warns_and_keeps_the_concave_tiers_as_footnotes() {
    let design = authored_session().design;
    assert_eq!(
        design.export_warnings(),
        [ManufacturabilityWarning::ConcaveTiersOmittedFromExport { count: 2 }],
        "the export dialog is told two tiers are left out of the tier list"
    );
    let solved = design.solve().expect("the fixture solves");
    let schedule = design.to_asc_schedule_for_export(&solved);
    assert_eq!(
        schedule.tiers.len(),
        design.tiers.len(),
        "a concave tier is never an .asc tier"
    );
    let footnotes = schedule.footnotes.join("\n");
    for tier in &design.concave_tiers {
        assert!(
            footnotes.contains(&tier.name),
            "{} survives as a footnote",
            tier.name
        );
        assert!(footnotes.contains(&tier.second_line_fields().join("  ")));
    }
    // A planar design has nothing to warn about and writes the plain schedule.
    let mut flat = design;
    flat.concave_tiers.clear();
    assert!(flat.export_warnings().is_empty());
}
