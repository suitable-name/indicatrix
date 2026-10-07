//! The New Design dialog's template gallery and options: the Rust side of
//! `ui/models/templates.slint`'s `TemplateGalleryModel`.
//!
//! The template data itself lives in `indicatrix_cut_core::templates` (nine entries, each
//! a statically verified closed solid) and the decisions in `indicatrix_editor::templates`
//! (the grouped cards, which gears a template allows, how a choice becomes a design);
//! this module is the glue:
//!
//! - [`setup_template_gallery`] pushes the cards, in the dialog's three sections ("Shapes",
//!   "Round variants and teaching designs", the blank design), once at startup. Every card
//!   carries its template index, so regrouping never changes what an index means.
//! - [`GalleryDialog`] answers the dialog's callbacks: what it opened on
//!   ([`GalleryDialog::shown`]), what a picked card offers ([`GalleryDialog::template_chosen`]:
//!   only the gears the template's facets still land on, the template's own material as the
//!   default, the "designed for" line), what a picked material means for the angles
//!   ([`GalleryDialog::material_chosen`]), and what Create asks for
//!   ([`GalleryDialog::choice`]). Building the design is `new_design.rs`'s job.
//! - The thumbnails ([`thumbnails`]) are drawn by a worker thread the first time the dialog
//!   opens and set on their cards as they arrive ([`set_thumbnail`]).
//!
//! # Every gallery entry is selectable
//!
//! `TemplateCardData::ready` is `true` for every card; `template_gallery.slint` has no "not
//! selectable yet" state to render.

mod thumbnails;

use crate::{
    EditorModel, MainWindow, TemplateCardData, TemplateGalleryModel,
    bridge::render_thread::RenderContext,
};
use indicatrix::optics::materials::GemMaterial;
use indicatrix_cut_core::{MaterialSelection, templates::TemplateSpec};
use indicatrix_editor::{
    material::{
        GEAR_PRESETS, design_material_index_from_name, design_material_name_from_index,
        design_material_options,
    },
    retarget::view::resolved_material_from_selection,
    templates::{
        GalleryCard, GalleryGroup, NewDesignChoice, gallery_cards, gallery_design, material_note,
        template_spec,
    },
};
use slint::{
    ComponentHandle as _, Image, Model as _, ModelRc, Rgba8Pixel, SharedPixelBuffer, SharedString,
    VecModel,
};
use std::{
    cell::OnceCell,
    sync::{Arc, Mutex},
};

/// The widest stone the dialog accepts, in millimetres -- the same bound
/// `new_design_dialog.slint` checks.
const MAX_STONE_WIDTH_MM: f64 = 200.0;

/// The template the dialog opens on when `EditorModel.new_template_index` names none:
/// "Standard Round Brilliant".
const DEFAULT_TEMPLATE_INDEX: i32 = 1;

/// A card as the Slint model holds it: no picture yet.
fn card_data(card: &GalleryCard) -> TemplateCardData {
    TemplateCardData {
        name: SharedString::from(card.name.as_str()),
        shape: SharedString::from(card.shape.as_str()),
        description: SharedString::from(card.description.as_str()),
        ready: true,
        template_index: card.template_index,
        designed_for: SharedString::from(card.designed_for()),
        thumbnail: Image::default(),
        has_thumbnail: false,
    }
}

/// The cards of one section, in section order.
fn card_rows(group: GalleryGroup) -> ModelRc<TemplateCardData> {
    ModelRc::new(VecModel::from(
        gallery_cards()
            .iter()
            .filter(|card| card.group == group)
            .map(card_data)
            .collect::<Vec<_>>(),
    ))
}

/// A Slint string list.
fn string_model(items: impl IntoIterator<Item = String>) -> ModelRc<SharedString> {
    ModelRc::new(VecModel::from(
        items
            .into_iter()
            .map(SharedString::from)
            .collect::<Vec<_>>(),
    ))
}

/// Pushes the template gallery into `TemplateGalleryModel` once, at startup: the
/// `indicatrix_editor::templates::gallery_cards` in their three sections, every one
/// `ready`, none with a picture yet (the worker draws those when the dialog first opens).
pub(in crate::gui::editor) fn setup_template_gallery(ui: &MainWindow) {
    let gallery = ui.global::<TemplateGalleryModel>();
    gallery.set_shapes(card_rows(GalleryGroup::Shapes));
    gallery.set_variants(card_rows(GalleryGroup::Variants));
    gallery.set_blank(card_rows(GalleryGroup::Blank));
}

/// Puts a finished thumbnail on the card of `template_index`. UI thread only (it makes the
/// `slint::Image`).
fn set_thumbnail(ui: &MainWindow, template_index: i32, pixels: &SharedPixelBuffer<Rgba8Pixel>) {
    let gallery = ui.global::<TemplateGalleryModel>();
    for rows in [gallery.get_shapes(), gallery.get_variants()] {
        for row in 0..rows.row_count() {
            let Some(mut card) = rows.row_data(row) else {
                continue;
            };
            if card.template_index == template_index {
                card.thumbnail = Image::from_rgba8(pixels.clone());
                card.has_thumbnail = true;
                rows.set_row_data(row, card);
            }
        }
    }
}

/// The templates (not the blank design) whose card has no picture yet, in card order --
/// shapes first.
fn missing_thumbnails(ui: &MainWindow) -> Vec<i32> {
    let gallery = ui.global::<TemplateGalleryModel>();
    [gallery.get_shapes(), gallery.get_variants()]
        .iter()
        .flat_map(|rows| rows.iter())
        .filter(|card| card.template_index >= 1 && !card.has_thumbnail)
        .map(|card| card.template_index)
        .collect()
}

/// The material combo's entries: the built-in materials, then the custom ones -- the
/// Edit tab's material list without its "(none)" first entry and its trailing "Custom
/// RI..." sentinel (a template is cut from a named material).
fn material_labels(custom: &[GemMaterial]) -> Vec<String> {
    let mut options = design_material_options(custom);
    options.pop();
    if !options.is_empty() {
        options.remove(0);
    }
    options
}

/// The selection for the material combo's `ui_index`, or `None` for an index outside the
/// list.
fn material_selection(ui_index: i32, custom: &[GemMaterial]) -> Option<MaterialSelection> {
    let options = design_material_options(custom);
    // The combo omits the list's "(none)" first entry, so its index is one behind.
    let name = design_material_name_from_index(ui_index.checked_add(1)?, &options)?;
    Some(MaterialSelection {
        name: Some(name),
        specific_gravity_override: None,
        refractive_index_override: None,
        body_color_override: None,
        body_color_bands_override: None,
        absorption_path_scale_override: None,
    })
}

/// The material combo's index of the template's own material (the first entry if the list
/// somehow lacks it).
fn default_material_index(spec: &TemplateSpec, custom: &[GemMaterial]) -> i32 {
    let options = design_material_options(custom);
    (design_material_index_from_name(Some(spec.default_material), &options) - 1).max(0)
}

/// The line under the material combo: what the material at `ui_index` means for the
/// template's facet angles. Empty for an index outside the list.
fn material_note_for(spec: &TemplateSpec, ui_index: i32, custom: &[GemMaterial]) -> String {
    let Some(selection) = material_selection(ui_index, custom) else {
        return String::new();
    };
    let label = selection
        .name
        .clone()
        .unwrap_or_else(|| "This material".to_string());
    let n_d = resolved_material_from_selection(&selection, custom).n_d;
    material_note(spec, &label, n_d)
}

/// `template_index` if it names a card of the dialog (a template, or `0` for the blank
/// design), else the round brilliant.
fn sanitized_template_index(template_index: i32) -> i32 {
    if template_index == 0 || template_spec(template_index).is_some() {
        template_index
    } else {
        DEFAULT_TEMPLATE_INDEX
    }
}

/// Fills the dialog's options for `template_index`: its gears (the template's own first),
/// its material as the default, and the lines that describe it. The blank design clears
/// them -- it uses the full form.
fn apply_template(ui: &MainWindow, template_index: i32, custom: &[GemMaterial]) {
    let gallery = ui.global::<TemplateGalleryModel>();
    let card = gallery_cards()
        .into_iter()
        .find(|card| card.template_index == template_index);
    if let (Some(card), Some(spec)) = (card, template_spec(template_index)) {
        gallery.set_gear_options(string_model(
            card.allowed_gears.iter().map(ToString::to_string),
        ));
        gallery.set_gear_index(0);
        let material_index = default_material_index(spec, custom);
        gallery.set_material_index(material_index);
        gallery.set_designed_for(SharedString::from(card.designed_for()));
        gallery.set_description(SharedString::from(card.description.as_str()));
        gallery.set_material_note(SharedString::from(material_note_for(
            spec,
            material_index,
            custom,
        )));
    } else {
        gallery.set_gear_options(string_model(Vec::new()));
        gallery.set_gear_index(0);
        gallery.set_designed_for(SharedString::new());
        gallery.set_description(SharedString::new());
        gallery.set_material_note(SharedString::new());
    }
}

/// Reads the stone width the dialog typed: empty is "not set" (`None`); otherwise a number
/// above 0 and at most [`MAX_STONE_WIDTH_MM`] millimetres.
///
/// # Errors
///
/// A sentence for the toast when the text is not such a number.
fn parse_stone_width(text: &str) -> Result<Option<f64>, String> {
    let text = text.trim();
    if text.is_empty() {
        return Ok(None);
    }
    match text.parse::<f64>() {
        Ok(mm) if mm.is_finite() && mm > 0.0 && mm <= MAX_STONE_WIDTH_MM => Ok(Some(mm)),
        _ => Err(format!(
            "The stone width must be a number above 0 and at most {MAX_STONE_WIDTH_MM} mm."
        )),
    }
}

/// What the dialog's Create asks for, validated: the template, the gear its combo offers at
/// `gear_index`, the material at `material_index` and the stone width typed.
///
/// # Errors
///
/// A sentence for the toast when the template, gear, material or width is not valid.
fn build_choice(
    template_index: i32,
    gear_index: i32,
    material_index: i32,
    width_text: &str,
    custom: Vec<GemMaterial>,
) -> Result<NewDesignChoice, String> {
    let spec =
        template_spec(template_index).ok_or_else(|| "Pick one of the shapes first.".to_string())?;
    let gear_teeth = usize::try_from(gear_index)
        .ok()
        .and_then(|index| spec.allowed_gears_among(&GEAR_PRESETS).get(index).copied())
        .ok_or_else(|| "Pick an index gear from the list.".to_string())?;
    let material = material_selection(material_index, &custom)
        .ok_or_else(|| "Pick a material from the list.".to_string())?;
    let girdle_diameter_mm = parse_stone_width(width_text)?;
    Ok(NewDesignChoice {
        template_index,
        gear_teeth,
        material,
        girdle_diameter_mm,
        custom_materials: custom,
    })
}

/// The dialog's template side: answers `TemplateGalleryModel`'s callbacks. One per window,
/// shared by the callback closures.
pub(in crate::gui::editor) struct GalleryDialog {
    /// For the custom materials the combo lists after the built-in ones.
    render_ctx: Arc<Mutex<RenderContext>>,
    /// The thumbnail worker, started the first time the dialog opens.
    thumbnails: OnceCell<thumbnails::Thumbnails>,
}

impl GalleryDialog {
    pub(in crate::gui::editor) fn new(render_ctx: &Arc<Mutex<RenderContext>>) -> Self {
        Self {
            render_ctx: Arc::clone(render_ctx),
            thumbnails: OnceCell::new(),
        }
    }

    /// The custom materials the render thread holds.
    fn custom_materials(&self) -> Vec<GemMaterial> {
        RenderContext::lock(&self.render_ctx)
            .custom_materials
            .as_ref()
            .clone()
    }

    /// The dialog opened: lists the materials, fills the options for the selected
    /// template and asks the worker for the thumbnails still missing.
    pub(in crate::gui::editor) fn shown(&self, ui: &MainWindow) {
        let custom = self.custom_materials();
        ui.global::<TemplateGalleryModel>()
            .set_material_options(string_model(material_labels(&custom)));
        let editor = ui.global::<EditorModel>();
        let template_index = sanitized_template_index(editor.get_new_template_index());
        editor.set_new_template_index(template_index);
        apply_template(ui, template_index, &custom);
        self.thumbnails
            .get_or_init(|| thumbnails::Thumbnails::spawn(ui.as_weak()))
            .request(missing_thumbnails(ui));
    }

    /// A card was picked.
    pub(in crate::gui::editor) fn template_chosen(&self, ui: &MainWindow, template_index: i32) {
        apply_template(
            ui,
            sanitized_template_index(template_index),
            &self.custom_materials(),
        );
    }

    /// The material combo changed: says what that means for the selected template's angles.
    pub(in crate::gui::editor) fn material_chosen(&self, ui: &MainWindow, material_index: i32) {
        let template_index = ui.global::<EditorModel>().get_new_template_index();
        if let Some(spec) = template_spec(template_index) {
            ui.global::<TemplateGalleryModel>()
                .set_material_note(SharedString::from(material_note_for(
                    spec,
                    material_index,
                    &self.custom_materials(),
                )));
        }
    }

    /// What Create asks for -- see [`build_choice`].
    ///
    /// # Errors
    ///
    /// A sentence for the toast when the choice is not valid.
    pub(in crate::gui::editor) fn choice(
        &self,
        template_index: i32,
        gear_index: i32,
        material_index: i32,
        width_text: &str,
    ) -> Result<NewDesignChoice, String> {
        build_choice(
            template_index,
            gear_index,
            material_index,
            width_text,
            self.custom_materials(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use indicatrix_cut_core::templates::TEMPLATES;
    use indicatrix_editor::templates::{AngleAdaptation, create_from_template};

    fn garnet() -> GemMaterial {
        let mut material = GemMaterial::diamond();
        material.name = "MyGarnet".to_string();
        material
    }

    fn names(rows: &ModelRc<TemplateCardData>) -> Vec<(i32, String)> {
        rows.iter()
            .map(|card| (card.template_index, card.name.to_string()))
            .collect()
    }

    #[test]
    fn the_three_sections_hold_the_cards_in_order() {
        let shapes = names(&card_rows(GalleryGroup::Shapes));
        assert_eq!(
            shapes.iter().map(|(index, _)| *index).collect::<Vec<_>>(),
            vec![1, 6, 7, 8, 9]
        );
        assert_eq!(shapes[1].1, "Oval Brilliant");
        let variants = names(&card_rows(GalleryGroup::Variants));
        assert_eq!(
            variants.iter().map(|(index, _)| *index).collect::<Vec<_>>(),
            vec![2, 3, 4, 5]
        );
        assert_eq!(
            names(&card_rows(GalleryGroup::Blank)),
            vec![(0, "Empty".to_string())]
        );
    }

    #[test]
    fn a_new_card_has_no_picture_and_says_what_it_was_designed_for() {
        let card = card_rows(GalleryGroup::Shapes)
            .row_data(1)
            .expect("the oval is the second shape");
        assert!(card.ready);
        assert!(!card.has_thumbnail);
        assert!(
            card.designed_for.starts_with("Designed for Sapphire"),
            "{}",
            card.designed_for
        );
        let blank = card_rows(GalleryGroup::Blank)
            .row_data(0)
            .expect("the blank card");
        assert!(blank.designed_for.is_empty());
    }

    #[test]
    fn the_material_list_leaves_out_none_and_the_custom_ri_entry() {
        let plain = material_labels(&[]);
        assert_eq!(plain.first().map(String::as_str), Some("Diamond"));
        assert!(
            plain
                .iter()
                .all(|name| name != "(none)" && !name.contains("Custom RI"))
        );
        let with_custom = material_labels(&[garnet()]);
        assert_eq!(with_custom.len(), plain.len() + 1);
        assert_eq!(with_custom.last().map(String::as_str), Some("MyGarnet"));
    }

    #[test]
    fn a_material_index_names_the_material_at_that_position() {
        let custom = [garnet()];
        let labels = material_labels(&custom);
        for (index, label) in labels.iter().enumerate() {
            let selection = material_selection(i32::try_from(index).unwrap(), &custom)
                .unwrap_or_else(|| panic!("index {index} ({label}) has no selection"));
            assert_eq!(selection.name.as_deref(), Some(label.as_str()));
            assert_eq!(selection.refractive_index_override, None);
        }
        assert!(material_selection(-1, &custom).is_none());
        assert!(material_selection(i32::try_from(labels.len()).unwrap(), &custom).is_none());
        assert!(material_selection(i32::MAX, &custom).is_none());
    }

    #[test]
    fn every_template_defaults_to_its_own_material() {
        for custom in [Vec::new(), vec![garnet()]] {
            let labels = material_labels(&custom);
            for spec in TEMPLATES {
                let index = default_material_index(spec, &custom);
                assert_eq!(
                    labels
                        .get(usize::try_from(index).unwrap())
                        .map(String::as_str),
                    Some(spec.default_material),
                    "{}",
                    spec.name
                );
            }
        }
    }

    #[test]
    fn the_note_under_the_material_says_whether_the_angles_change() {
        let oval = template_spec(6).expect("the oval");
        let labels = material_labels(&[]);
        let position = |name: &str| {
            i32::try_from(labels.iter().position(|label| label == name).expect(name)).unwrap()
        };
        let own = material_note_for(oval, position("Sapphire"), &[]);
        assert!(own.contains("used as authored"), "{own}");
        let other = material_note_for(oval, position("Quartz"), &[]);
        assert!(other.contains("adapted"), "{other}");
        assert_eq!(material_note_for(oval, -1, &[]), "");
    }

    #[test]
    fn the_dialog_opens_on_a_real_card() {
        assert_eq!(sanitized_template_index(0), 0);
        assert_eq!(sanitized_template_index(7), 7);
        for stale in [-1, 10, 99, i32::MIN] {
            assert_eq!(sanitized_template_index(stale), DEFAULT_TEMPLATE_INDEX);
        }
    }

    #[test]
    fn the_stone_width_is_empty_or_a_sensible_number() {
        assert_eq!(parse_stone_width(""), Ok(None));
        assert_eq!(parse_stone_width("  "), Ok(None));
        assert_eq!(parse_stone_width("6.5"), Ok(Some(6.5)));
        assert_eq!(parse_stone_width(" 12 "), Ok(Some(12.0)));
        assert_eq!(parse_stone_width("200"), Ok(Some(200.0)));
        for bad in ["0", "-3", "abc", "6,5", "201", "inf", "NaN", "1e400"] {
            assert!(parse_stone_width(bad).is_err(), "{bad:?} was accepted");
        }
    }

    #[test]
    fn a_choice_carries_the_gear_the_combo_shows() {
        let oval = template_spec(6).expect("the oval");
        let gears = oval.allowed_gears_among(&GEAR_PRESETS);
        assert_eq!(gears, vec![96, 80, 64]);
        for (index, gear) in gears.iter().enumerate() {
            let choice = build_choice(
                6,
                i32::try_from(index).unwrap(),
                default_material_index(oval, &[]),
                "7.5",
                Vec::new(),
            )
            .expect("a valid choice");
            assert_eq!(choice.gear_teeth, *gear);
            assert_eq!(choice.template_index, 6);
            assert_eq!(choice.girdle_diameter_mm, Some(7.5));
            assert_eq!(choice.material.name.as_deref(), Some("Sapphire"));
        }
    }

    #[test]
    fn a_choice_outside_the_lists_is_refused_with_a_sentence() {
        for (template, gear, material, width) in [
            (0, 0, 0, "6.5"),
            (42, 0, 0, "6.5"),
            (6, 3, 0, "6.5"),
            (6, -1, 0, "6.5"),
            (6, 0, -1, "6.5"),
            (6, 0, 500, "6.5"),
            (6, 0, 0, "wide"),
        ] {
            let error = build_choice(template, gear, material, width, Vec::new())
                .err()
                .unwrap_or_else(|| {
                    panic!("({template}, {gear}, {material}, {width:?}) was accepted")
                });
            assert!(error.ends_with('.'), "{error}");
        }
    }

    /// The dialog's defaults build the template as authored, for every card.
    #[test]
    fn the_default_choice_for_every_card_builds_the_design() {
        for spec_index in 1..=i32::try_from(TEMPLATES.len()).unwrap() {
            let spec = template_spec(spec_index).expect("a template");
            let choice = build_choice(
                spec_index,
                0,
                default_material_index(spec, &[]),
                "6.5",
                Vec::new(),
            )
            .expect("the defaults are a valid choice");
            let created = create_from_template(&choice).expect("the defaults build");
            assert_eq!(
                created.adaptation,
                AngleAdaptation::NotNeeded,
                "{}",
                spec.name
            );
            assert_eq!(created.session.design.girdle_diameter_mm, Some(6.5));
            assert_eq!(created.session.design.meta.gear_teeth, 96);
        }
    }
}
