//! The New Design template gallery's Rust-side glue. The template data itself
//! lives in `indicatrix_cut_core::templates` (five entries, each a
//! statically-verified closed solid -- see that module's doc comment for why
//! five, not a dozen); this module's only job is handing that data to
//! `ui/models/templates.slint`'s `TemplateGalleryModel` for the gallery grid to
//! render.
//!
//! # Every gallery entry is selectable
//!
//! `new_design_dialog.slint`'s "Create" button calls
//! `EditorModel.new_design_create(..., template_index)`, handled by
//! `gui::editor::callbacks::tier_actions::do_new_design_create`: that
//! function's template dispatch now indexes
//! `indicatrix_cut_core::templates::TEMPLATES` at `template_index - 1` for
//! every `template_index >= 1` (index 1 is "Standard Round Brilliant",
//! `TEMPLATES[0]`, matching this gallery's own display order), so every card
//! [`setup_template_gallery`] lists below is reachable, not only the first.
//! [`TemplateCardData::ready`] is `true` for every entry as a result.
//! `template_gallery.slint` has no "not selectable yet" state to render.

use crate::{MainWindow, TemplateCardData};
use slint::{ComponentHandle as _, ModelRc, SharedString, VecModel};

/// Pushes the built-in template gallery into `TemplateGalleryModel.templates`
/// once, at startup: `indicatrix_editor::templates::template_cards` (index 0
/// "Empty", matching `new_design_dialog.slint`'s own combo model, then every
/// `indicatrix_cut_core::templates::TEMPLATES` entry in order, every one `ready`),
/// mapped to the Slint card struct.
pub(in crate::gui::editor) fn setup_template_gallery(ui: &MainWindow) {
    let cards: Vec<TemplateCardData> = indicatrix_editor::templates::template_cards()
        .into_iter()
        .map(|card| TemplateCardData {
            name: SharedString::from(card.name),
            shape: SharedString::from(card.shape),
            description: SharedString::from(card.description),
            ready: card.ready,
        })
        .collect();
    ui.global::<crate::TemplateGalleryModel>()
        .set_templates(ModelRc::new(VecModel::from(cards)));
}
