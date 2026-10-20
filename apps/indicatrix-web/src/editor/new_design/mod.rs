//! The New Design dialog (the desktop's `new_design_dialog.slint` and
//! `tier_actions::new_design`): the grouped template gallery (Shapes, Round Variants and
//! Teaching Designs, Blank) over `indicatrix_editor::templates::gallery_cards`, and two ways
//! to create.
//!
//! - **A template card** (`GalleryModel.selected-index >= 1`): the dialog offers the gears the
//!   template's facets still land on, the material (the template's own by default, with the
//!   "Designed for Sapphire (n 1.768)" line and what the choice means for the angles) and an
//!   optional stone width. Create builds the design with `create_from_template`, which remaps
//!   the gear and, when the material's index differs from the one the template was designed
//!   for, adapts the pavilion angles through the retarget engine (kept only when the result is
//!   valid; the toast says which).
//! - **Empty** (index 0): every `FreshDesignSpec` field -- preform shape and size, index gear,
//!   symmetry, mirror, starting material -- parsed from the form.
//!
//! The dialog is open while `GalleryModel.open` is true. Creating over unsaved changes goes
//! through the Save / Discard / Cancel guard (`editor::unsaved`); the dialog closes once the
//! new design is installed. The template logic that needs no window is in [`choice`].

mod choice;

use crate::{
    AppWindow, GalleryModel, NewDesignModel, TemplateCard,
    app::{
        Ctx,
        persist::schedule_save,
        push::{MessageKind, push_all, show_message},
        solve::auto_solve,
        state::{DesignSource, DesignState},
    },
    editor::unsaved::confirm_discard,
};
use choice::{
    build_choice, default_material_index, material_labels, material_note_for,
    sanitized_template_index,
};
use indicatrix::optics::materials::GemMaterial;
use indicatrix_cut_core::FreshDesignSpec;
use indicatrix_editor::{
    EditorSession,
    loading::{parse_new_design_form, parse_preform_form},
    material::{builtin_preset_names, gear_choice_to_teeth},
    templates::{GalleryCard, NewDesignChoice, create_from_template, gallery_cards, template_spec},
};
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};
use std::rc::Rc;

/// A cylinder preform's side count from the dialog: fixed, independent of the gear (the
/// desktop's `FIXED_CYLINDER_PREFORM_SIDES`).
const FIXED_CYLINDER_PREFORM_SIDES: usize = 96;

/// A Slint string list.
fn string_model(items: impl IntoIterator<Item = String>) -> ModelRc<SharedString> {
    ModelRc::new(VecModel::from(
        items
            .into_iter()
            .map(SharedString::from)
            .collect::<Vec<_>>(),
    ))
}

/// Fills the gallery once, at start-up: the cards in the dialog's section order (Shapes,
/// Round Variants and Teaching Designs, Blank) into `GalleryModel.templates`, and beside
/// them -- as parallel lists, indexed like the cards -- each card's template index, section
/// code and "Designed for ..." line into `NewDesignModel`.
pub fn push_cards(ui: &AppWindow) {
    let cards = gallery_cards();
    let templates: Vec<TemplateCard> = cards
        .iter()
        .map(|card| TemplateCard {
            name: card.name.as_str().into(),
            shape: card.shape.as_str().into(),
            description: card.description.as_str().into(),
        })
        .collect();
    ui.global::<GalleryModel>()
        .set_templates(Rc::new(VecModel::from(templates)).into());
    let model = ui.global::<NewDesignModel>();
    model.set_card_template_index(ModelRc::new(VecModel::from(
        cards.iter().map(|c| c.template_index).collect::<Vec<_>>(),
    )));
    model.set_card_group(ModelRc::new(VecModel::from(
        cards.iter().map(|c| c.group.code()).collect::<Vec<_>>(),
    )));
    model.set_card_designed_for(string_model(cards.iter().map(GalleryCard::designed_for)));
}

/// The spec the Empty form's fields describe, or the message naming the field that is wrong.
fn spec_from_fields(model: &NewDesignModel<'_>) -> Result<FreshDesignSpec, String> {
    let gear_teeth = gear_choice_to_teeth(model.get_gear_index(), &model.get_gear_custom_text())?;
    let preform = parse_preform_form(
        model.get_preform_shape_index(),
        &model.get_preform_half_width(),
        &model.get_preform_length_over_width(),
        &model.get_preform_depth(),
        FIXED_CYLINDER_PREFORM_SIDES,
    )?;
    parse_new_design_form(
        gear_teeth,
        preform,
        &model.get_symmetry_order_text(),
        model.get_mirror(),
        model.get_material_index(),
    )
}

/// Fills the template-specific fields for card `template_index`: its gears (the template's
/// own first), its material as the default, and the lines that describe it. The blank design
/// clears them -- it uses the full form.
fn apply_template(ui: &AppWindow, template_index: i32, custom: &[GemMaterial]) {
    let model = ui.global::<NewDesignModel>();
    let card = gallery_cards()
        .into_iter()
        .find(|card| card.template_index == template_index);
    if let (Some(card), Some(spec)) = (card, template_spec(template_index)) {
        model.set_template_gear_options(string_model(
            card.allowed_gears.iter().map(ToString::to_string),
        ));
        model.set_template_gear_index(0);
        let material_index = default_material_index(spec, custom);
        model.set_template_material_index(material_index);
        model.set_designed_for(card.designed_for().into());
        model.set_material_note(material_note_for(spec, material_index, custom).into());
    } else {
        model.set_template_gear_options(string_model(Vec::new()));
        model.set_template_gear_index(0);
        model.set_designed_for(SharedString::new());
        model.set_material_note(SharedString::new());
    }
}

/// The dialog opened: lists the materials (built-in, then the vault's custom ones) and fills
/// the options of the selected card.
fn dialog_shown(ctx: &Ctx) {
    let Some(ui) = ctx.ui.upgrade() else {
        return;
    };
    let custom = ctx.state.borrow().custom_materials.clone();
    ui.global::<NewDesignModel>()
        .set_template_material_options(string_model(material_labels(&custom)));
    let gallery = ui.global::<GalleryModel>();
    let template_index = sanitized_template_index(gallery.get_selected_index());
    gallery.set_selected_index(template_index);
    apply_template(&ui, template_index, &custom);
}

/// A card was picked: refills the template-specific fields for it.
fn template_chosen(ctx: &Ctx, template_index: i32) {
    let Some(ui) = ctx.ui.upgrade() else {
        return;
    };
    let custom = ctx.state.borrow().custom_materials.clone();
    apply_template(&ui, sanitized_template_index(template_index), &custom);
}

/// The material combo changed: says what that means for the selected template's angles.
fn material_chosen(ctx: &Ctx, material_index: i32) {
    let Some(ui) = ctx.ui.upgrade() else {
        return;
    };
    let template_index = ui.global::<GalleryModel>().get_selected_index();
    if let Some(spec) = template_spec(template_index) {
        let custom = ctx.state.borrow().custom_materials.clone();
        ui.global::<NewDesignModel>()
            .set_material_note(material_note_for(spec, material_index, &custom).into());
    }
}

/// Create: validates the dialog's fields (template or Empty form), then replaces the design
/// after the unsaved-changes guard. A form error surfaces before any confirmation, so a
/// create that cannot succeed never asks to discard anything.
fn create(ctx: &Ctx) {
    let Some(ui) = ctx.ui.upgrade() else {
        return;
    };
    let template_index = ui.global::<GalleryModel>().get_selected_index();
    if template_index >= 1 {
        let model = ui.global::<NewDesignModel>();
        let custom = ctx.state.borrow().custom_materials.clone();
        match build_choice(
            template_index,
            model.get_template_gear_index(),
            model.get_template_material_index(),
            &model.get_stone_width_text(),
            &custom,
        ) {
            Ok(choice) => {
                let c = ctx.clone();
                confirm_discard(ctx, move || install_template(&c, &choice));
            }
            Err(message) => show_message(ctx, MessageKind::Error, &message),
        }
        return;
    }
    match spec_from_fields(&ui.global::<NewDesignModel>()) {
        Ok(spec) => {
            let c = ctx.clone();
            confirm_discard(ctx, move || install_empty(&c, spec));
        }
        Err(message) => show_message(ctx, MessageKind::Error, &message),
    }
}

/// Swaps `session` in as the open design, closes the dialog and runs what a new design owes
/// the rest of the page. Returns nothing; the caller toasts last so its message stays.
fn install_session(ctx: &Ctx, session: EditorSession, name: &str) {
    {
        let mut app = ctx.state.borrow_mut();
        app.replace_design(DesignState::new(
            session,
            DesignSource::Template(name.to_string()),
        ));
        // A design the user just created is unsaved until the first save.
        if let Some(design) = app.design.as_mut() {
            design.session.mark_unsaved();
        }
    }
    if let Some(ui) = ctx.ui.upgrade() {
        ui.global::<GalleryModel>().set_open(false);
        push_all(&ui, &ctx.state.borrow());
    }
    auto_solve(ctx);
    schedule_save(ctx);
    show_message(ctx, MessageKind::Success, &format!("New design: {name}."));
    // The guided walkthrough's first step waits for exactly this event.
    crate::editor::guide::notify(ctx, indicatrix_editor::guide::NEW_DESIGN_CREATED);
}

/// Installs the Empty design from the form, once nothing unsaved is at stake.
fn install_empty(ctx: &Ctx, spec: FreshDesignSpec) {
    // Seeded directly on the fresh design, not through the history: a design's starting
    // state is not an edit.
    install_session(ctx, EditorSession::from_template(spec, 0), "Empty");
}

/// Installs the design a template choice asks for, once nothing unsaved is at stake: built
/// by `create_from_template` (gear remapped, width set, angles adapted to the material). A
/// refused build keeps the dialog open with its values and says why; what the material did
/// to the angles is toasted last.
fn install_template(ctx: &Ctx, choice: &NewDesignChoice) {
    let created = match create_from_template(choice) {
        Ok(created) => created,
        Err(message) => {
            show_message(ctx, MessageKind::Error, &message);
            return;
        }
    };
    let name = template_spec(choice.template_index).map_or("Template", |spec| spec.name);
    let mut session = created.session;
    // The template's name becomes the design's title line, so the title survives a save and
    // a reload (a restored design has no template source to name it), as in the mobile app.
    if session.design.meta.headers.is_empty() {
        session.design.meta.headers = vec![name.to_owned()];
    }
    install_session(ctx, session, name);
    if let Some(message) = created.adaptation.message() {
        let kind = if created.adaptation.adapted() {
            MessageKind::Info
        } else {
            MessageKind::Warning
        };
        show_message(ctx, kind, &message);
    }
}

/// Fills the Empty form's material list and registers the dialog's callbacks.
pub fn wire(ui: &AppWindow, ctx: &Ctx) {
    let model = ui.global::<NewDesignModel>();
    model.set_material_options(string_model(builtin_preset_names()));
    let c = ctx.clone();
    model.on_create(move || create(&c));
    let c = ctx.clone();
    model.on_dialog_shown(move || dialog_shown(&c));
    let c = ctx.clone();
    model.on_template_chosen(move |template_index| template_chosen(&c, template_index));
    let c = ctx.clone();
    model.on_material_chosen(move |material_index| material_chosen(&c, material_index));
}
