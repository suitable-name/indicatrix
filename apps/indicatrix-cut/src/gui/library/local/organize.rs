//! Organizing an existing local design: ignore/un-ignore, rename, shape edit, and
//! delete -- the "Organize" half of `gui::library::local` (see this group's own
//! `mod.rs`); import and export live in their own sibling modules.

use super::helpers::refresh_after_library_change;
use crate::{
    DiagramDetailData, LibraryModel, MainWindow,
    bridge::{
        library::source::LibrarySource,
        render_thread::{PlanesOwner, RenderContext},
    },
    gui::show_toast,
};
use indicatrix::geometry::cuts::StandardGemCuts;
use indicatrix_vault::{db::sqlite::Database, model::metadata_update::MetadataUpdate};
use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel};
use std::sync::{Arc, Mutex};

/// Wires the per-design "Ignore"/"Un-ignore" context-menu toggle --
/// `diagram_list.slint`'s `ContextMenuArea`, alongside "Generate Previews"/"Compute
/// Tilt Curves". Refuses outright while a remote library is being browsed, same
/// reasoning as [`setup_rename_callback`]'s own doc comment: `Database::set_diagram_ignored`
/// is a local-database write, and the remote library protocol has no write request for
/// it even if this wanted to send one.
pub fn setup_ignore_toggle_callback(
    ui: &MainWindow,
    db: &Arc<Mutex<Database>>,
    source: &Arc<Mutex<LibrarySource>>,
) {
    let db_ignore = Arc::clone(db);
    let source_ignore = Arc::clone(source);
    let ui_weak = ui.as_weak();
    ui.global::<LibraryModel>()
        .on_toggle_ignored(move |id: i32| {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            if source_ignore
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .is_remote()
            {
                show_toast(
                    &ui,
                    "Switch to the local library to ignore a design.",
                    "error",
                );
                return;
            }
            // The currently-visible list already carries whether this row is ignored
            // (`DiagramItem.ignored`, populated by `gui::library::search::fetch_diagram_list` --
            // see that function's own doc comment for how, since
            // `indicatrix_vault::model::entry::DiagramListItem` itself has no such field).
            // Reading it back off the list avoids a database round trip just to answer
            // "is this currently ignored" before flipping it. A row not found in the
            // currently-visible list (a stale click racing a refresh) falls back to
            // `true` -- ignoring is the more common direction this action is used for,
            // and a mistaken ignore is one more click to undo.
            let currently_ignored = ui
                .global::<LibraryModel>()
                .get_diagram_list()
                .iter()
                .find(|item| item.id == id)
                .is_some_and(|item| item.ignored);
            let new_ignored = !currently_ignored;
            let result = {
                let db = db_ignore
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                db.set_diagram_ignored(i64::from(id), new_ignored)
            };
            match result {
                Ok(()) => {
                    show_toast(
                        &ui,
                        if new_ignored {
                            "Design ignored."
                        } else {
                            "Design un-ignored."
                        },
                        "success",
                    );
                    refresh_after_library_change(&ui, &db_ignore, &source_ignore);
                }
                Err(e) => show_toast(&ui, &format!("Failed to update ignored flag: {e}"), "error"),
            }
        });
}

/// Wires up the detail header's inline rename (pencil icon -> `rename_diagram`).
///
/// Refuses outright while a remote library is being browsed: the selected entry id in
/// that case names a row in the REMOTE server's catalogue, not this process's local
/// database, and the library protocol is read-only in any case (see
/// `indicatrix_net::library`'s module doc comment -- there is no write request to send even
/// if this wanted to act on the remote row instead). Renaming a LOCAL row while
/// browsing remote is not offered either -- the visible list, and so the selection,
/// is the remote one.
pub fn setup_rename_callback(
    ui: &MainWindow,
    db: &Arc<Mutex<Database>>,
    source: &Arc<Mutex<LibrarySource>>,
) {
    let db_rename = Arc::clone(db);
    let source_rename = Arc::clone(source);
    let ui_weak = ui.as_weak();
    ui.global::<LibraryModel>()
        .on_rename_diagram(move |new_title: SharedString| {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            if source_rename
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .is_remote()
            {
                show_toast(
                    &ui,
                    "Switch to the local library to rename a design.",
                    "error",
                );
                return;
            }
            let entry_id = ui.global::<LibraryModel>().get_selected_entry_id();
            if entry_id < 0 {
                return;
            }
            let result = {
                let db = db_rename
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                db.rename_diagram_entry(i64::from(entry_id), &new_title)
            };
            match result {
                Ok(()) => {
                    let mut detail = ui.global::<LibraryModel>().get_current_detail();
                    detail.title = new_title;
                    ui.global::<LibraryModel>().set_current_detail(detail);
                    show_toast(&ui, "Renamed.", "success");
                    refresh_after_library_change(&ui, &db_rename, &source_rename);
                }
                Err(e) => show_toast(&ui, &format!("Rename failed: {e}"), "error"),
            }
        });
}

/// Sets `entry_id`'s shape to `new_shape`, called by [`setup_set_shape_callback`].
///
/// Edits exactly one column. `Database::update_diagram_metadata` issues a narrow
/// `UPDATE diagram_details SET ...` naming only the fields handed to it, so every
/// other column keeps whatever it already held -- unlike
/// `Database::save_diagram_detail`, which fully REPLACES a design's detail row and
/// would therefore zero anything the caller failed to carry across.
///
/// The rest of the [`MetadataUpdate`] below is read straight back out of the design's
/// current record and passed through unchanged, so the net effect is `shape` and
/// nothing else. In particular the proportions are NOT recomputed: they are measured
/// data (see `super::import::apply_measured_metadata`, which derives them once at
/// import from the design's own geometry), and re-deriving them on an unrelated shape
/// edit would be both wasted work and a chance to disagree with the stored value.
///
/// This used to be considerably worse. `FullDiagramRecord` was once a strict subset
/// of `FacetDiagramDetail`, missing a dozen columns, so a read-modify-write through
/// `save_diagram_detail` silently erased them -- including the very proportions and
/// symmetry values the import step had just computed. The workaround was to re-parse
/// the design's original `.asc` attachment on every shape edit to recover them, with a
/// lossy fallback for designs that had no attachment. Both the subset gap and the
/// workaround are gone: the record now carries every column, and this method writes
/// only what it is given.
fn set_diagram_shape(db: &Database, entry_id: i64, new_shape: &str) -> Result<(), String> {
    let full = db
        .get_diagram_full(entry_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "Diagram detail not found.".to_string())?;

    let update = MetadataUpdate {
        shape: Some(new_shape.to_string()),
        // Everything below is this design's existing value, round-tripped unchanged.
        designer_info: full.designer_info,
        refractive_index: full.refractive_index,
        index_gear: full.index_gear,
        facets_count: full.facets_count,
        symmetry_order: full.symmetry_order,
        mirror_symmetry: full.mirror_symmetry,
        lw_ratio: full.lw_ratio,
        hw_ratio: full.hw_ratio,
        cw_ratio: full.cw_ratio,
        pw_ratio: full.pw_ratio,
        volume: full.volume,
    };
    db.update_diagram_metadata(entry_id, &update)
        .map_err(|e| e.to_string())
}

/// Builds the shape picker's option list and the index within it that matches
/// `current` (the selected design's own `shape`, `None`/empty for an unclassified
/// import). Called from `gui::library::detail::load_diagram_detail` every time a LOCAL
/// design is opened, so the dropdown always reflects this design and the library's
/// present vocabulary.
///
/// `Database::get_unique_shapes` already unions the seeded `DEFAULT_SHAPES` with
/// every distinct scraped value actually present, alphabetically -- this only adds
/// one more thing: if `current` itself isn't in that union for some reason (should be
/// rare -- it would mean this design's own `shape` value was written by something
/// other than `save_diagram_detail`/the seeded vocabulary), it's inserted too, so
/// opening the picker can never silently drop the design's existing value out of the
/// list before the user has touched anything.
pub fn build_shape_picker_options(
    db: &Database,
    current: Option<&str>,
) -> (Vec<SharedString>, i32) {
    let mut shapes = db.get_unique_shapes().unwrap_or_default();
    let current = current.unwrap_or("").trim();
    if !current.is_empty() && !shapes.iter().any(|s| s == current) {
        shapes.push(current.to_string());
        shapes.sort();
    }
    let index = if current.is_empty() {
        -1
    } else {
        shapes
            .iter()
            .position(|s| s == current)
            .map_or(-1, |i| i as i32)
    };
    (shapes.into_iter().map(SharedString::from).collect(), index)
}

/// Wires up the detail header's shape picker (pencil icon next to the "Shape:" chip,
/// same idiom as [`setup_rename_callback`]'s title pencil) -> `set_shape`.
///
/// Refuses outright while a remote library is being browsed -- see
/// `setup_rename_callback`'s own doc comment; `detail_header.slint` additionally only
/// shows the pencil for `root.detail.is_local`, so this guard is a backstop, not the
/// only thing standing between a remote-sourced id and the local database.
pub fn setup_set_shape_callback(
    ui: &MainWindow,
    db: &Arc<Mutex<Database>>,
    source: &Arc<Mutex<LibrarySource>>,
) {
    let db_shape = Arc::clone(db);
    let source_shape = Arc::clone(source);
    let ui_weak = ui.as_weak();
    ui.global::<LibraryModel>()
        .on_set_shape(move |new_shape: SharedString| {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            if source_shape
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .is_remote()
            {
                show_toast(
                    &ui,
                    "Switch to the local library to change a design's shape.",
                    "error",
                );
                return;
            }
            let entry_id = ui.global::<LibraryModel>().get_selected_entry_id();
            if entry_id < 0 {
                return;
            }
            let result = {
                let db = db_shape
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                set_diagram_shape(&db, i64::from(entry_id), new_shape.as_str())
            };
            match result {
                Ok(()) => {
                    let mut detail = ui.global::<LibraryModel>().get_current_detail();
                    detail.shape = new_shape;
                    ui.global::<LibraryModel>().set_current_detail(detail);
                    show_toast(&ui, "Shape updated.", "success");
                    refresh_after_library_change(&ui, &db_shape, &source_shape);
                }
                Err(e) => show_toast(&ui, &format!("Shape update failed: {e}"), "error"),
            }
        });
}

/// Wires up the "Add tag..." context-menu entry (`diagram_list.slint`, CAD audit
/// item 190) -> `add_tag_to_entry`. Creates the tag if it doesn't already exist
/// (case-insensitively) and attaches it to `entry_id` -- see
/// `Database::add_tag_to_entry`'s own doc comment for the exact create-or-reuse
/// rule. A blank name (whitespace-only) is rejected with a toast rather than
/// silently creating an empty tag.
///
/// Refuses outright while a remote library is being browsed, same reasoning as
/// [`setup_rename_callback`]'s own doc comment.
pub fn setup_add_tag_callback(
    ui: &MainWindow,
    db: &Arc<Mutex<Database>>,
    source: &Arc<Mutex<LibrarySource>>,
) {
    let db_tag = Arc::clone(db);
    let source_tag = Arc::clone(source);
    let ui_weak = ui.as_weak();
    ui.global::<LibraryModel>().on_add_tag_to_entry(
        move |entry_id: i32, tag_name: SharedString| {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            if source_tag
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .is_remote()
            {
                show_toast(&ui, "Switch to the local library to tag a design.", "error");
                return;
            }
            let result = {
                let db = db_tag
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                db.add_tag_to_entry(i64::from(entry_id), tag_name.as_str())
            };
            match result {
                Ok(tag) => {
                    show_toast(&ui, &format!("Tagged \"{}\".", tag.name), "success");
                    refresh_after_library_change(&ui, &db_tag, &source_tag);
                }
                Err(e) => show_toast(&ui, &format!("Failed to add tag: {e}"), "error"),
            }
        },
    );
}

/// Wires up a tag chip's remove control (`diagram_list.slint`) -> `remove_tag_from_entry`.
/// Takes the tag's NAME rather than its id -- the card only ever carries
/// `DiagramItem.tags: [string]` (see `gui::library::diagram_list::
/// sync_tag_vocabulary_to_ui`'s own doc comment for why the Slint side never
/// handles a tag id directly), so this resolves the name to an id via
/// `Database::tag_id_by_name` itself. A name that no longer resolves (the tag was
/// deleted from elsewhere between the chip rendering and this click) is a silent
/// no-op, not an error -- there is nothing left to remove.
///
/// Refuses outright while a remote library is being browsed, same reasoning as
/// [`setup_rename_callback`]'s own doc comment.
pub fn setup_remove_tag_callback(
    ui: &MainWindow,
    db: &Arc<Mutex<Database>>,
    source: &Arc<Mutex<LibrarySource>>,
) {
    let db_untag = Arc::clone(db);
    let source_untag = Arc::clone(source);
    let ui_weak = ui.as_weak();
    ui.global::<LibraryModel>().on_remove_tag_from_entry(
        move |entry_id: i32, tag_name: SharedString| {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            if source_untag
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .is_remote()
            {
                show_toast(
                    &ui,
                    "Switch to the local library to untag a design.",
                    "error",
                );
                return;
            }
            let result = {
                let db = db_untag
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                match db.tag_id_by_name(tag_name.as_str()) {
                    Ok(Some(tag_id)) => db.remove_tag_from_entry(i64::from(entry_id), tag_id),
                    Ok(None) => Ok(()),
                    Err(e) => Err(e),
                }
            };
            match result {
                Ok(()) => refresh_after_library_change(&ui, &db_untag, &source_untag),
                Err(e) => show_toast(&ui, &format!("Failed to remove tag: {e}"), "error"),
            }
        },
    );
}

/// Wires up the detail header's delete confirm -> `delete_diagram`: removes the
/// entry (cascading to its detail/angle-settings/attached-files rows, see
/// `Database::delete_diagram_entry`) and clears the now-stale detail/3D-viewport
/// state.
///
/// Refuses outright while a remote library is being browsed -- see
/// `setup_rename_callback`'s own doc comment; the same reasoning applies verbatim
/// (and matters even more here, since a mistaken delete against the wrong local row by
/// a remote-sourced id would be destructive, not just cosmetically wrong).
pub fn setup_delete_callback(
    ui: &MainWindow,
    db: &Arc<Mutex<Database>>,
    source: &Arc<Mutex<LibrarySource>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
) {
    let db_delete = Arc::clone(db);
    let source_delete = Arc::clone(source);
    let render_ctx_delete = render_ctx.clone();
    let ui_weak = ui.as_weak();
    ui.global::<LibraryModel>().on_delete_diagram(move || {
        let Some(ui) = ui_weak.upgrade() else {
            return;
        };
        if source_delete
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_remote()
        {
            show_toast(
                &ui,
                "Switch to the local library to delete a design.",
                "error",
            );
            return;
        }
        let entry_id = ui.global::<LibraryModel>().get_selected_entry_id();
        if entry_id < 0 {
            return;
        }
        let result = {
            let db = db_delete
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            db.delete_diagram_entry(i64::from(entry_id))
        };
        match result {
            Ok(()) => {
                ui.global::<LibraryModel>()
                    .set_current_detail(DiagramDetailData {
                        id: -1,
                        title: SharedString::default(),
                        url: SharedString::default(),
                        designer: SharedString::default(),
                        shape: SharedString::default(),
                        gear: SharedString::default(),
                        facets: SharedString::default(),
                        lw_ratio: SharedString::default(),
                        ri: SharedString::default(),
                        volume: SharedString::default(),
                        competition: SharedString::default(),
                        image_name: SharedString::default(),
                        has_image: false,
                        is_local: false,
                        hw_ratio: SharedString::default(),
                        cw_ratio: SharedString::default(),
                        pw_ratio: SharedString::default(),
                        symmetry_order: SharedString::default(),
                        mirror_symmetry: false,
                    });
                ui.global::<LibraryModel>().set_selected_entry_id(-1);
                ui.global::<LibraryModel>()
                    .set_current_angles(ModelRc::new(VecModel::from(Vec::new())));
                ui.global::<LibraryModel>()
                    .set_current_files(ModelRc::new(VecModel::from(Vec::new())));
                {
                    let mut ctx = render_ctx_delete
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    // CAD audit items 58 and 145: deleting a row used to reset the
                    // traced geometry to the demo brilliant no matter what was on
                    // screen, so deleting an unrelated catalogue row threw away the
                    // design being edited. Only reset when the deleted row is the
                    // one whose planes are actually loaded.
                    let owns_deleted_row = matches!(
                        ctx.planes_owner,
                        PlanesOwner::Catalogue { entry_id: owned } if owned == i64::from(entry_id)
                    );
                    if owns_deleted_row {
                        ctx.claim_active_planes(
                            std::sync::Arc::new(StandardGemCuts::standard_round_brilliant()),
                            None,
                            PlanesOwner::Builtin,
                        );
                        ctx.dirty = true;
                    }
                }
                show_toast(&ui, "Diagram deleted.", "info");
                refresh_after_library_change(&ui, &db_delete, &source_delete);
            }
            Err(e) => show_toast(&ui, &format!("Delete failed: {e}"), "error"),
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gui::library::local::helpers::test_support::{open_temp_db, temp_db_path_for_test};
    use indicatrix_vault::model::detail::FacetDiagramDetail;

    /// `set_diagram_shape` must edit `shape` and NOTHING else.
    ///
    /// `indicatrix-vault` already proves `update_diagram_metadata` writes only the
    /// columns it is handed; this proves the wiring on THIS side hands it all of them.
    /// Omitting one field from the `MetadataUpdate` literal would compile fine and
    /// silently blank that column on every shape edit -- exactly the class of data loss
    /// the re-parse workaround this function replaced existed to avoid.
    #[test]
    fn set_diagram_shape_changes_only_the_shape() {
        let path = temp_db_path_for_test("set_shape");
        let db_arc = open_temp_db(&path);
        let db = db_arc.lock().expect("lock temp db");

        let entry = indicatrix_vault::model::entry::FacetDiagramEntry {
            title: "Shape edit probe".to_string(),
            url: "local://shape_probe.asc".to_string(),
            design_id: String::new(),
        };
        let detail = FacetDiagramDetail {
            refractive_index: Some("1.762".to_string()),
            index_gear: Some("96".to_string()),
            facets_count: Some("57".to_string()),
            symmetry_order: Some("8".to_string()),
            mirror_symmetry: Some(true),
            lw_ratio: Some("1.234".to_string()),
            hw_ratio: Some("0.617".to_string()),
            cw_ratio: Some("0.145".to_string()),
            pw_ratio: Some("0.431".to_string()),
            volume: Some("0.187".to_string()),
            designer_info: Some("Somebody; Some Journal".to_string()),
            shape: Some("Oval".to_string()),
            ..FacetDiagramDetail::default()
        };
        let entry_id = db
            .save_diagram_entry(&entry, indicatrix_vault::local::LOCAL_SOURCE_ID)
            .expect("save entry");
        db.save_diagram_detail(&detail, entry_id)
            .expect("save detail");

        set_diagram_shape(&db, entry_id, "Cushion").expect("set shape");

        let after = db
            .get_diagram_full(entry_id)
            .expect("read back")
            .expect("row exists");
        assert_eq!(after.shape.as_deref(), Some("Cushion"), "shape must change");
        // Every other field must be exactly what it was. `pw_ratio`/`cw_ratio`/
        // `hw_ratio`/`symmetry_order`/`mirror_symmetry` are the ones the old
        // `FullDiagramRecord` could not carry at all, so they are the load-bearing
        // assertions here.
        assert_eq!(after.refractive_index.as_deref(), Some("1.762"));
        assert_eq!(after.index_gear.as_deref(), Some("96"));
        assert_eq!(after.facets_count.as_deref(), Some("57"));
        assert_eq!(after.symmetry_order.as_deref(), Some("8"));
        assert_eq!(after.mirror_symmetry, Some(true));
        assert_eq!(after.lw_ratio.as_deref(), Some("1.234"));
        assert_eq!(after.hw_ratio.as_deref(), Some("0.617"));
        assert_eq!(after.cw_ratio.as_deref(), Some("0.145"));
        assert_eq!(after.pw_ratio.as_deref(), Some("0.431"));
        assert_eq!(after.volume.as_deref(), Some("0.187"));
        assert_eq!(
            after.designer_info.as_deref(),
            Some("Somebody; Some Journal")
        );

        drop(db);
        drop(db_arc);
        std::fs::remove_file(&path).ok();
    }
}
