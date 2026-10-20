//! The open document: where the design came from, its display name, what a save needs
//! to write it back the way the desktop would, opening and creating designs, and the
//! autosave.
//!
//! Everything here works on bytes and names. The front end asks the platform for files
//! ([`crate::platform::Platform`]) and hands the bytes to [`open_picked`]; it asks for
//! the bytes of a save ([`save_bytes`], [`asc_bytes`]) and hands them to the platform.
//! The logic is a port of the web app's `io::load`, `io::save`, `app::persist` and
//! `editor::new_design` over the same shared crates, so a design opened or saved here is
//! the design the desktop and the browser open and save.
//!
//! | Module | What it does |
//! |---|---|
//! | `open` | [`open_picked`]: classify, pair, convert, load, install |
//! | `create` | [`new_from_template`], [`new_from_form`], [`template_cards`] |
//! | `save` | [`save_bytes`], [`asc_bytes`], [`mark_saved`] |
//! | `autosave` | [`autosave`], [`restore`] |
//!
//! | `library` | [`open_library`], [`save_to_library`], [`record_library_recent`] |
//!
//! A design opened from the library database (or saved to it) has the origin
//! [`DocumentOrigin::Library`]; "Save" then writes it back to its entry.

mod autosave;
mod create;
mod library;
#[cfg(test)]
mod library_tests;
mod open;
mod save;
mod stamp;
#[cfg(test)]
mod tests;

use indicatrix::{geometry::stone_metrics::ExternalProportions, optics::materials::GemMaterial};
use indicatrix_cut_core::{
    Design,
    native::{AttachmentBlob, CustomMaterialSnapshot, DesignMetadata},
};
use indicatrix_editor::{EditorSession, files::suggested_asc_file_name};
use indicatrix_web_core::custom_material::{builtin_name_conflict, builtin_name_message};
use serde::{Deserialize, Serialize};

use crate::app_state::AppState;

pub use autosave::{RestoreReport, autosave, restore};
pub use create::{
    NewDesignForm, PreformShapeChoice, RawNewDesignForm, TemplateCard, new_from_form,
    new_from_spec, new_from_template, template_cards, warm_template_cards,
};
pub use library::{
    LibrarySavePlan, finish_library_save, library_handle, open_library, plan_library_save,
    record_library_recent, save_to_library, write_library_save,
};
pub use open::{OpenError, OpenReport, open_picked};
pub use save::{SaveError, SavedFile, asc_bytes, mark_saved, save_bytes};

/// The title shown for a design with no title line and no file name.
const UNTITLED: &str = "Untitled Design";

/// Gives unsaved documents one durable variant family before their first variant.
/// Later file/library saves reuse this metadata UUID; no duplicate catalogue entry
/// is inserted merely to hold variants.
pub fn ensure_document_id(state: &mut AppState) {
    if state.document.metadata.id.trim().is_empty() {
        state.document.metadata = stamp::stamped(&state.document.metadata);
        state.session.mark_unsaved();
        state.operations.invalidate();
    }
}

/// Where the current design came from.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DocumentOrigin {
    /// The start-up design nobody has touched or replaced yet.
    #[default]
    Startup,
    /// A design created from the Empty form.
    New,
    /// A design created from a template card (`index` is the session template index:
    /// `0` is Empty, `n >= 1` is `TEMPLATES[n - 1]`).
    Template {
        /// The template's session index.
        index: usize,
    },
    /// A design read from a file, or saved to one.
    Document {
        /// The file name the user saw.
        display_name: String,
    },
    /// A design opened from a library entry, or saved to one; "Save" overwrites the entry.
    Library {
        /// The entry's id in the library database.
        entry_id: i64,
    },
}

/// Where the current design came from and everything a save needs besides the design
/// itself. See the module docs.
#[derive(Debug, Default)]
pub struct DocumentState {
    /// Where the design came from.
    pub origin: DocumentOrigin,
    /// The file name the user saw when the design was opened or last saved.
    pub display_name: Option<String>,
    /// The `.asc` name the design is recorded under (`round.asc`), so a save proposes the
    /// same stem. `None` for a design never saved or opened.
    pub asc_filename: Option<String>,
    /// The opened `.asc`'s exact text, kept to regenerate the catalogue `.asc` on save.
    pub original_asc_text: Option<String>,
    /// The printed proportions a design file carried in its `[source]` table.
    pub printed_proportions: Option<ExternalProportions>,
    /// The custom-material snapshot a design file carried, written back on the next save
    /// while the design still names a custom material.
    pub custom_material: Option<CustomMaterialSnapshot>,
    /// The `[meta]` table a design file carried (empty for any other source), written
    /// back on the next save with a fresh `modified_at`; unknown keys included.
    pub metadata: DesignMetadata,
    /// The attachments a design file carried, kept byte for byte and written back.
    pub attachments: Vec<AttachmentBlob>,
    /// Custom materials restored from opened design files, by name; consulted by every
    /// export and offered after the built-ins in the material picker.
    pub custom_materials: Vec<GemMaterial>,
    /// What the autosave last wrote, see the `autosave` module.
    pub(crate) autosaved: autosave::Mark,
}

impl DocumentState {
    /// The `.asc` name a save proposes for `design`
    /// ([`indicatrix_editor::files::suggested_asc_file_name`]).
    #[must_use]
    pub fn save_asc_name(&self, design: &Design) -> String {
        suggested_asc_file_name(self.asc_filename.as_deref(), &design.meta.headers)
    }

    /// The custom-material snapshot a native save should carry: [`Self::custom_material`]
    /// while `design`'s material is a custom one, `None` once it names a built-in (a
    /// snapshot left over from a material the design no longer uses must not be written
    /// under the new name).
    #[must_use]
    pub fn custom_snapshot(&self, design: &Design) -> Option<&CustomMaterialSnapshot> {
        let name = design.material.name.as_deref()?;
        if builtin_name_conflict(name).is_some() {
            return None;
        }
        self.custom_material.as_ref()
    }

    /// Whether the Design page has a design to show: the design has tiers, or the user
    /// opened or created one (an Empty design is a design too).
    #[must_use]
    pub fn has_design(&self, design: &Design) -> bool {
        !design.tiers.is_empty() || self.origin != DocumentOrigin::Startup
    }

    /// Registers (or replaces, by case-insensitive name) a custom material.
    ///
    /// # Errors
    /// The refusal text when `material`'s name is a built-in material's: a custom
    /// material under it would silently shadow the built-in everywhere it is selected.
    pub fn register_custom_material(&mut self, material: GemMaterial) -> Result<(), String> {
        if let Some(builtin) = builtin_name_conflict(&material.name) {
            return Err(builtin_name_message(&material.name, &builtin));
        }
        if let Some(existing) = self
            .custom_materials
            .iter_mut()
            .find(|known| known.name.eq_ignore_ascii_case(&material.name))
        {
            *existing = material;
        } else {
            self.custom_materials.push(material);
        }
        Ok(())
    }
}

/// The title to show for the open design: its title line, else the name of the file it
/// was opened from or saved to (without extension), else "Untitled Design".
#[must_use]
pub fn display_title(state: &AppState) -> String {
    let title = state.design_title();
    if title != UNTITLED {
        return title;
    }
    state
        .document
        .display_name
        .as_deref()
        .map(|name| name.rsplit_once('.').map_or(name, |(stem, _)| stem))
        .filter(|stem| !stem.is_empty())
        .map_or(title, str::to_owned)
}

/// The custom-material snapshot a native save of `state`'s design carries.
///
/// Resolve by name from the session catalogue, which includes restored files and
/// newly created materials. The opened document's unkeyed recipe may belong to a
/// material that the working design no longer uses.
#[must_use]
pub fn save_snapshot(state: &AppState) -> Option<CustomMaterialSnapshot> {
    let design = &state.session.design;
    let name = design.material.name.as_deref()?;
    if builtin_name_conflict(name).is_some() {
        return None;
    }
    state.props.snapshot_for(name).cloned()
}

/// Every custom material of the session, by name, without duplicates: those the opened
/// files brought (`document`) and those made in the Stone tab (`props`).
#[must_use]
pub fn custom_catalogue(state: &AppState) -> Vec<GemMaterial> {
    // The live, editable catalogue supersedes recipes retained by older documents.
    let mut all = state.props.custom_materials.clone();
    for material in &state.document.custom_materials {
        if !all
            .iter()
            .any(|known| known.name.eq_ignore_ascii_case(&material.name))
        {
            all.push(material.clone());
        }
    }
    all
}

/// Replaces the open design: continues the generation counter (so any job in flight
/// sees the change), forgets the old solve and the tier selection, and installs
/// `document` with the custom materials the old document had registered plus `restored`.
pub(crate) fn install(
    state: &mut AppState,
    session: EditorSession,
    mut document: DocumentState,
    restored: Option<GemMaterial>,
) {
    let mut catalogue = std::mem::take(&mut state.document.custom_materials);
    document.custom_materials.append(&mut catalogue);
    if let Some(material) = restored {
        // The Stone tab's catalogue (`design_props`) lists and selects custom materials,
        // so a material restored from the file is registered there too, with the
        // snapshot the next save must carry.
        if let Err(message) = state
            .props
            .register_custom(material.clone(), document.custom_material.clone())
        {
            tracing::warn!(%message, "a restored custom material was not offered in the Stone tab");
        }
        if let Err(message) = document.register_custom_material(material) {
            tracing::warn!(%message, "a restored custom material was not registered");
        }
    }
    state.session.replace_with(session);
    state.operations.replace_document();
    state.document = document;
    // `invalidate` (not `Default`) bumps the solve revision, so a solve still running
    // for the old design is recognised as stale when it lands.
    state.solve.invalidate();
    state.selected_tier = None;
}

/// Closes the open design: the session returns to the empty start-up design (its undo
/// history goes with the old one), the document forgets where the design came from, the
/// old solve and tier selection are dropped, and every long operation for the old design is
/// cancelled (the same replacement `install` does for a new design). The autosave is
/// removed so the design does not come back at the next start. Unsaved changes are the
/// caller's question; this discards them.
///
/// Afterwards [`DocumentState::has_design`] is false and the Design page offers
/// "Start a Design" again. Custom materials registered in the session stay available.
pub fn close_design(state: &mut AppState) {
    install(
        state,
        EditorSession::fresh(),
        DocumentState::default(),
        None,
    );
    autosave::remove_autosave(state);
}

/// Seconds since the Unix epoch (0 when the clock is before it).
pub(crate) fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| i64::try_from(elapsed.as_secs()).unwrap_or(0))
}
