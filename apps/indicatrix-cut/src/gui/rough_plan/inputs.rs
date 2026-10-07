//! Input parsing, validation, and filter snapshotting for rough planning.

use super::{cut_rows::fmt_num, host::Host, session::MaterialChoice};
use crate::{
    LibraryModel, MainWindow, RoughPlanModel, RoughPlannerWindow,
    gui::library::search::{read_id_filter, read_local_only, read_range_filter, read_tag_filter},
};
use indicatrix::optics::materials::GemMaterial;
use indicatrix_cut_core::{material::built_in_specific_gravity, rough_plan::PlanSettings};
use indicatrix_vault::{
    db::sqlite::{Database, DisplayFilters, SortOrder},
    model::filter::RangeFilter,
};
use slint::{ComponentHandle, Model, SharedString};
use std::{
    rc::Rc,
    sync::{Mutex, PoisonError},
};
use tracing::warn;

/// The text of option `index` of a dropdown model, or `"All"` for its "all" entry.
pub fn selected_option(
    options: &impl Model<Data = SharedString>,
    index: i32,
    all_label: &str,
) -> String {
    let text = usize::try_from(index)
        .ok()
        .and_then(|i| options.row_data(i))
        .unwrap_or_default();
    if text.as_str() == all_label {
        "All".to_string()
    } else {
        text.to_string()
    }
}

/// The library panel's filter state, read on the UI thread so the query itself can run
/// on any thread. The same reads and the same query as
/// `gui::library::search::current_filtered_entry_ids`, split in two.
#[derive(Debug, Clone, PartialEq)]
pub struct FilterSnapshot {
    /// The library search text.
    pub search: String,
    /// The shape filter ("All" for none).
    pub shape: String,
    /// The gear filter ("All" for none).
    pub gear: String,
    /// The numeric range filters.
    pub range: RangeFilter,
    /// Only designs stored in the local library.
    pub local_only: bool,
    /// The tag filter, if one is active.
    pub tag_name: Option<String>,
    /// The ids the library panel is restricted to, if it is.
    pub id_filter: Option<Vec<i64>>,
}

impl FilterSnapshot {
    /// Reads the filter state off the window (UI thread only).
    pub fn read(ui: &MainWindow) -> Self {
        let library = ui.global::<LibraryModel>();
        Self {
            search: library.get_search_text().to_string(),
            shape: selected_option(
                &library.get_shape_options(),
                library.get_selected_shape_index(),
                "All Shapes",
            ),
            gear: selected_option(
                &library.get_gear_options(),
                library.get_selected_gear_index(),
                "All Gears",
            ),
            range: read_range_filter(ui),
            local_only: read_local_only(ui),
            tag_name: read_tag_filter(ui),
            id_filter: read_id_filter(ui),
        }
    }

    /// The ids of the designs the snapshot's filters match, sorted. The designs excluded
    /// from the planner are among them (the library lists them too); the run and the
    /// counts subtract those.
    pub fn query(&self, db: &Mutex<Database>) -> Result<Vec<i64>, String> {
        let db = db.lock().unwrap_or_else(PoisonError::into_inner);
        let tag_filter = self
            .tag_name
            .as_deref()
            .and_then(|name| db.tag_id_by_name(name).ok().flatten());
        let filters = DisplayFilters {
            // `matching_entry_ids` walks in catalogue order regardless.
            order: SortOrder::CatalogueOrder,
            local_only: self.local_only,
            tag_filter,
            id_filter: self.id_filter.as_deref(),
        };
        db.matching_entry_ids(&self.search, &self.shape, &self.gear, &self.range, filters)
            .map(sorted_unique)
            .map_err(|e| e.to_string())
    }
}

/// `ids` sorted and without duplicates.
pub fn sorted_unique(mut ids: Vec<i64>) -> Vec<i64> {
    ids.sort_unstable();
    ids.dedup();
    ids
}

/// The most the kerf, allowance and skin fields take, in mm. A saw kerf, a polishing
/// allowance or a rough skin beyond this is a typing slip, and a huge value would only
/// make the planner find nothing.
pub const MAX_LOSS_MM: f64 = 50.0;

/// The most the minimum-width field takes, in mm.
pub const MAX_MIN_WIDTH_MM: f64 = 1000.0;

/// Shown when the rough has no size yet.
pub const NO_SIZE_MESSAGE: &str = "Enter the rough size in mm.";

/// Shown while a cut field holds text that is not a number.
pub const BAD_CUT_FIELD_MESSAGE: &str = "A cut field does not hold a number yet.";

/// Shown when no material with a known specific gravity is picked.
pub const PICK_MATERIAL_MESSAGE: &str = "Pick a material with a known specific gravity.";

/// Starts the message of a Fit to weight that could not scale the model.
pub const FIT_FAILED_PREFIX: &str = "Fit to weight failed:";

/// Shown when the model was edited while a mesh rough was being scaled for Fit to weight.
pub const MODEL_CHANGED_MESSAGE: &str =
    "The model changed while it was being scaled. Press Fit to weight again.";

/// Whether `text` is a message about the inputs (the rough size, a cut field, the plan
/// form, the material, a Fit to weight that did not go through) as opposed to one about a
/// run or the library. Such a message is out of date as soon as an input changes; the
/// others are not.
#[must_use]
pub fn is_input_message(text: &str) -> bool {
    const FIELD_PREFIXES: [&str; 7] = [
        "Scan plan time limit",
        "Kerf",
        "Allowance",
        "Skin",
        "The skin",
        "Minimum width",
        "Weighed carat",
    ];
    matches!(
        text,
        NO_SIZE_MESSAGE | BAD_CUT_FIELD_MESSAGE | PICK_MATERIAL_MESSAGE | MODEL_CHANGED_MESSAGE
    ) || text.starts_with(FIT_FAILED_PREFIX)
        || FIELD_PREFIXES.iter().any(|prefix| text.starts_with(prefix))
}

/// The plan form as typed: everything [`PlanForm::parse`] needs, readable off the window.
#[derive(Debug, Clone, Default)]
pub struct FormFields {
    /// The saw kerf, in mm.
    pub kerf_mm: String,
    /// The allowance per side, in mm.
    pub allowance_mm: String,
    /// The rough skin, in mm.
    pub skin_mm: String,
    /// The minimum stone width, in mm.
    pub min_width_mm: String,
    /// The weighed carat ("" when not weighed).
    pub weighed_ct: String,
    /// The stone-count limit as the spin box holds it.
    pub count: i32,
    /// The picked row of the material list (negative for none).
    pub material_index: i32,
}

impl FormFields {
    /// Reads the form off the window (UI thread only).
    pub fn read(window: &RoughPlannerWindow) -> Self {
        let model = window.global::<RoughPlanModel>();
        Self {
            kerf_mm: model.get_kerf_mm().to_string(),
            allowance_mm: model.get_allowance_mm().to_string(),
            skin_mm: model.get_skin_mm().to_string(),
            min_width_mm: model.get_min_width_mm().to_string(),
            weighed_ct: model.get_weighed_ct().to_string(),
            count: model.get_count(),
            material_index: model.get_material_index(),
        }
    }
}

/// A validated plan form.
#[derive(Debug, Clone, PartialEq)]
pub struct PlanForm {
    /// Stone-count limit, losses, minimum width and specific gravity.
    pub settings: PlanSettings,
    /// The chosen material's name.
    pub material_name: String,
    /// The weighed carat of the rough, if one was entered.
    pub weighed_ct: Option<f64>,
}

/// Parses one number typed into a field that is measured in `unit` ("mm", "degrees"):
/// "4,5" and "4.5" both read as 4.5. The unit is named in the message for text that is
/// not a number, so a field of angles never asks for millimetres.
///
/// # Errors
///
/// Returns the message for `label`'s field when the text is not a finite number.
pub fn parse_with_unit(text: &str, label: &str, unit: &str) -> Result<f64, String> {
    let value: f64 = text
        .trim()
        .replace(',', ".")
        .parse()
        .map_err(|_| format!("{label} must be a number in {unit}."))?;
    if value.is_finite() {
        Ok(value)
    } else {
        Err(format!("{label} must be a finite number."))
    }
}

/// Parses one millimetre field ("4,5" and "4.5" both read as 4.5).
pub fn parse_mm(text: &str, label: &str) -> Result<f64, String> {
    parse_with_unit(text, label, "mm")
}

/// Parses the weighed carat field: empty is "not weighed".
///
/// # Errors
///
/// Returns the message for text that is not a positive number.
pub fn parse_weighed(text: &str) -> Result<Option<f64>, String> {
    if text.trim().is_empty() {
        return Ok(None);
    }
    let value: f64 = text
        .trim()
        .replace(',', ".")
        .parse()
        .map_err(|_| "Weighed carat must be a number in ct.".to_string())?;
    if value.is_finite() && value > 0.0 {
        Ok(Some(value))
    } else {
        Err("Weighed carat must be greater than 0 ct.".to_string())
    }
}

/// Fails when `value` is above `limit_mm`, naming the field.
fn check_at_most(value: f64, limit_mm: f64, label: &str) -> Result<(), String> {
    if value > limit_mm {
        Err(format!("{label} must be at most {limit_mm} mm."))
    } else {
        Ok(())
    }
}

impl PlanForm {
    /// Validates the typed form against the material list. `Err` is the message for
    /// `error_text`. The rough itself is validated by the model, not here.
    ///
    /// # Errors
    ///
    /// Returns the message for the first invalid field.
    pub fn parse(fields: &FormFields, choices: &[MaterialChoice]) -> Result<Self, String> {
        let kerf_mm = parse_mm(&fields.kerf_mm, "Kerf")?;
        let allowance_mm = parse_mm(&fields.allowance_mm, "Allowance")?;
        let skin_mm = parse_mm(&fields.skin_mm, "Skin")?;
        let min_width_mm = parse_mm(&fields.min_width_mm, "Minimum width")?;
        if kerf_mm < 0.0 || allowance_mm < 0.0 || skin_mm < 0.0 {
            return Err("Kerf, allowance and skin cannot be negative.".to_string());
        }
        if min_width_mm <= 0.0 {
            return Err("Minimum width must be greater than 0 mm.".to_string());
        }
        check_at_most(kerf_mm, MAX_LOSS_MM, "Kerf")?;
        check_at_most(allowance_mm, MAX_LOSS_MM, "Allowance")?;
        check_at_most(skin_mm, MAX_LOSS_MM, "Skin")?;
        check_at_most(min_width_mm, MAX_MIN_WIDTH_MM, "Minimum width")?;
        let weighed_ct = parse_weighed(&fields.weighed_ct)?;
        let choice = usize::try_from(fields.material_index)
            .ok()
            .and_then(|i| choices.get(i))
            .ok_or_else(|| PICK_MATERIAL_MESSAGE.to_string())?;
        let count = u8::try_from(fields.count.clamp(1, 99)).unwrap_or(1);
        Ok(Self {
            settings: PlanSettings {
                count,
                kerf_mm,
                allowance_mm,
                skin_mm,
                min_width_mm,
                specific_gravity: choice.specific_gravity,
            },
            material_name: choice.name.clone(),
            weighed_ct,
        })
    }
}

/// Fails when the form's losses leave nothing of a rough with these `extents` (the
/// bounding-box sides, in mm): the skin on both sides of the smallest side, the skin and
/// the allowance together on both sides of it, or a minimum width that is larger than
/// the rough's largest side.
///
/// # Errors
///
/// Returns the message for `error_text`, naming the fields that cannot be met.
pub fn check_skin(settings: &PlanSettings, extents: [f64; 3]) -> Result<(), String> {
    let smallest = extents.iter().copied().fold(f64::INFINITY, f64::min);
    let largest = extents.iter().copied().fold(0.0_f64, f64::max);
    if 2.0 * settings.skin_mm >= smallest {
        return Err("The skin allowance leaves nothing of the rough.".to_string());
    }
    if 2.0 * (settings.skin_mm + settings.allowance_mm) >= smallest {
        return Err(format!(
            "Skin plus allowance ({} mm a side) leaves nothing of the rough: its smallest side is {} mm.",
            fmt_num(settings.skin_mm + settings.allowance_mm, 2),
            fmt_num(smallest, 2)
        ));
    }
    if settings.min_width_mm > largest {
        return Err(format!(
            "Minimum width ({} mm) is larger than the rough's largest side ({} mm).",
            fmt_num(settings.min_width_mm, 2),
            fmt_num(largest, 2)
        ));
    }
    Ok(())
}

/// The plan settings, the material's name and the weighed carat as the form holds them
/// now (what a run, or a saved plan, is made of besides the model).
///
/// # Errors
///
/// Returns the message for the first invalid field.
pub(super) fn current_settings(
    host: &Rc<Host>,
) -> Result<(PlanSettings, String, Option<f64>), String> {
    let fields = FormFields::read(&host.window);
    let form = PlanForm::parse(&fields, &host.session.borrow().choices)?;
    Ok((form.settings, form.material_name, form.weighed_ct))
}

/// The built-in materials with a known specific gravity. Needs no database.
pub fn built_in_choices() -> Vec<MaterialChoice> {
    let mut choices: Vec<MaterialChoice> = Vec::new();
    for material in GemMaterial::all_materials() {
        if let Some(sg) = built_in_specific_gravity(&material.name)
            && !choices
                .iter()
                .any(|c| c.name.eq_ignore_ascii_case(&material.name))
        {
            choices.push(MaterialChoice {
                name: material.name.clone(),
                specific_gravity: sg.representative,
            });
        }
    }
    choices
}

/// Appends the catalogue materials `rows` (name and recorded specific gravity) to
/// `choices`. A row without a usable specific gravity is skipped, and a name that is
/// already taken gets a " (catalogue)" suffix.
pub fn append_custom_materials<S: Into<f64>>(
    choices: &mut Vec<MaterialChoice>,
    rows: impl IntoIterator<Item = (String, Option<S>)>,
) {
    for (name, gravity) in rows {
        let sg: Option<f64> = gravity.map(S::into);
        let Some(sg) = sg else {
            continue;
        };
        if !sg.is_finite() || sg <= 0.0 {
            continue;
        }
        let taken = choices.iter().any(|c| c.name.eq_ignore_ascii_case(&name));
        let name = if taken {
            format!("{name} (catalogue)")
        } else {
            name
        };
        choices.push(MaterialChoice {
            name,
            specific_gravity: sg,
        });
    }
}

/// The materials the dialog offers: built-ins with a known specific gravity, then
/// catalogue materials with a recorded one. Never a guessed number.
///
/// Waits for the database lock, so it must not run on the UI thread.
pub fn material_choices(db: &Mutex<Database>) -> Vec<MaterialChoice> {
    let mut choices = built_in_choices();
    let rows = db
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get_custom_materials();
    match rows {
        Ok(rows) => append_custom_materials(
            &mut choices,
            rows.into_iter().map(|row| (row.name, row.specific_gravity)),
        ),
        Err(e) => warn!("Rough planner: could not read the catalogue materials: {e}"),
    }
    choices
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn millimetre_fields_accept_a_decimal_comma_and_reject_junk() {
        assert_eq!(parse_mm(" 4,5 ", "X"), Ok(4.5));
        assert_eq!(parse_mm("0.30", "Kerf"), Ok(0.3));
        assert!(parse_mm("", "X").is_err());
        assert!(parse_mm("abc", "X").is_err());
        assert!(parse_mm("inf", "X").is_err());
        assert!(parse_mm("NaN", "X").is_err());
    }

    #[test]
    fn the_weighed_carat_is_optional_but_must_be_positive_when_given() {
        assert_eq!(parse_weighed(""), Ok(None));
        assert_eq!(parse_weighed("  "), Ok(None));
        assert_eq!(parse_weighed("9,54"), Ok(Some(9.54)));
        assert!(parse_weighed("0").is_err());
        assert!(parse_weighed("-1").is_err());
        assert!(parse_weighed("heavy").is_err());
        assert!(parse_weighed("inf").is_err());
    }

    fn materials() -> Vec<MaterialChoice> {
        vec![
            MaterialChoice {
                name: "Corundum".to_string(),
                specific_gravity: 4.0,
            },
            MaterialChoice {
                name: "Quartz".to_string(),
                specific_gravity: 2.65,
            },
        ]
    }

    fn fields() -> FormFields {
        FormFields {
            kerf_mm: "0.30".to_string(),
            allowance_mm: "0,20".to_string(),
            skin_mm: "0.00".to_string(),
            min_width_mm: "1.00".to_string(),
            weighed_ct: String::new(),
            count: 12,
            material_index: 1,
        }
    }

    #[test]
    fn a_valid_form_gives_the_settings_the_material_and_the_weighed_carat() {
        let form = PlanForm::parse(
            &FormFields {
                weighed_ct: "9.54".to_string(),
                ..fields()
            },
            &materials(),
        )
        .expect("a valid form");
        assert_eq!(
            form.settings,
            PlanSettings {
                count: 12,
                kerf_mm: 0.3,
                allowance_mm: 0.2,
                skin_mm: 0.0,
                min_width_mm: 1.0,
                specific_gravity: 2.65,
            }
        );
        assert_eq!(form.material_name, "Quartz");
        assert_eq!(form.weighed_ct, Some(9.54));
    }

    #[test]
    fn the_stone_count_is_clamped_to_the_range_the_planner_takes() {
        for (typed, used) in [(0, 1), (-5, 1), (1, 1), (99, 99), (500, 99)] {
            let form = PlanForm::parse(
                &FormFields {
                    count: typed,
                    ..fields()
                },
                &materials(),
            )
            .expect("a valid form");
            assert_eq!(form.settings.count, used, "typed {typed}");
        }
    }

    #[test]
    fn every_invalid_field_is_named_in_the_message() {
        let cases = [
            (
                FormFields {
                    kerf_mm: "-0.1".to_string(),
                    ..fields()
                },
                "cannot be negative",
            ),
            (
                FormFields {
                    skin_mm: "-1".to_string(),
                    ..fields()
                },
                "cannot be negative",
            ),
            (
                FormFields {
                    min_width_mm: "0".to_string(),
                    ..fields()
                },
                "Minimum width must be greater than 0",
            ),
            (
                FormFields {
                    allowance_mm: "x".to_string(),
                    ..fields()
                },
                "Allowance must be a number",
            ),
            (
                FormFields {
                    weighed_ct: "-3".to_string(),
                    ..fields()
                },
                "Weighed carat",
            ),
            (
                FormFields {
                    material_index: 7,
                    ..fields()
                },
                "Pick a material",
            ),
            (
                FormFields {
                    material_index: -1,
                    ..fields()
                },
                "Pick a material",
            ),
        ];
        for (fields, expected) in cases {
            let message = PlanForm::parse(&fields, &materials()).unwrap_err();
            assert!(message.contains(expected), "{message} vs {expected}");
        }
    }

    #[test]
    fn the_built_in_materials_need_no_database_and_carry_a_positive_gravity() {
        let choices = built_in_choices();
        assert!(
            choices.iter().any(|c| c.name == "Quartz"),
            "the default material is offered"
        );
        assert!(
            choices
                .iter()
                .all(|c| c.specific_gravity.is_finite() && c.specific_gravity > 0.0)
        );
        for (i, a) in choices.iter().enumerate() {
            assert!(
                choices[i + 1..]
                    .iter()
                    .all(|b| !b.name.eq_ignore_ascii_case(&a.name)),
                "{} is listed once",
                a.name
            );
        }
    }

    #[test]
    fn catalogue_materials_skip_missing_gravity_and_rename_clashes() {
        let mut choices = materials();
        append_custom_materials(
            &mut choices,
            [
                ("Ruby glass".to_string(), Some(3.9_f32)),
                ("no gravity".to_string(), None),
                ("zero".to_string(), Some(0.0)),
                ("negative".to_string(), Some(-2.0)),
                ("not a number".to_string(), Some(f32::NAN)),
                ("quartz".to_string(), Some(2.7)),
            ],
        );
        let names: Vec<&str> = choices.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(
            names,
            ["Corundum", "Quartz", "Ruby glass", "quartz (catalogue)"]
        );
        assert!((choices[2].specific_gravity - 3.9).abs() < 1e-6);
    }

    #[test]
    fn a_skin_that_eats_a_whole_side_is_refused() {
        let settings = PlanSettings {
            skin_mm: 2.0,
            ..PlanSettings::default()
        };
        assert!(check_skin(&settings, [10.0, 8.0, 6.0]).is_ok());
        assert!(check_skin(&settings, [10.0, 4.0, 6.0]).is_err());
        assert!(check_skin(&settings, [10.0, 3.0, 6.0]).is_err());
    }

    #[test]
    fn the_skin_and_the_allowance_together_must_leave_some_of_the_smallest_side() {
        // Both sides of the smallest side (6 mm) lose 2 x (skin + allowance).
        let with_allowance = |allowance_mm| PlanSettings {
            allowance_mm,
            ..PlanSettings::default()
        };
        // 2 x 2.9 = 5.8 < 6 leaves 0.2 mm; 2 x 3 = 6 leaves nothing.
        assert!(check_skin(&with_allowance(2.9), [10.0, 8.0, 6.0]).is_ok());
        let message = check_skin(&with_allowance(3.0), [10.0, 8.0, 6.0]).unwrap_err();
        assert!(message.contains("Skin plus allowance"), "{message}");
        assert!(message.contains("6 mm"), "{message}");
        // The skin alone keeps its own message.
        let skin_only = PlanSettings {
            skin_mm: 3.0,
            allowance_mm: 0.0,
            ..PlanSettings::default()
        };
        let message = check_skin(&skin_only, [10.0, 8.0, 6.0]).unwrap_err();
        assert!(message.contains("skin allowance"), "{message}");
    }

    #[test]
    fn a_minimum_width_beyond_the_largest_side_is_refused() {
        let with_width = |min_width_mm| PlanSettings {
            min_width_mm,
            ..PlanSettings::default()
        };
        assert!(check_skin(&with_width(10.0), [10.0, 8.0, 6.0]).is_ok());
        let message = check_skin(&with_width(10.5), [10.0, 8.0, 6.0]).unwrap_err();
        assert!(message.starts_with("Minimum width"), "{message}");
        assert!(message.contains("10 mm"), "{message}");
    }

    #[test]
    fn the_upper_bounds_are_named_and_enforced_at_the_limit() {
        let parse = |fields| PlanForm::parse(&fields, &materials());
        // The limits themselves are valid.
        let at_limits = FormFields {
            kerf_mm: "50".to_string(),
            allowance_mm: "50".to_string(),
            skin_mm: "50".to_string(),
            min_width_mm: "1000".to_string(),
            ..fields()
        };
        assert!(parse(at_limits).is_ok());
        // Just above, and absurdly above ("1e300" is a finite number), each field is
        // refused by name.
        for typed in ["50.01", "1e300"] {
            let cases = [
                (
                    FormFields {
                        kerf_mm: typed.to_string(),
                        ..fields()
                    },
                    "Kerf must be at most 50 mm.",
                ),
                (
                    FormFields {
                        allowance_mm: typed.to_string(),
                        ..fields()
                    },
                    "Allowance must be at most 50 mm.",
                ),
                (
                    FormFields {
                        skin_mm: typed.to_string(),
                        ..fields()
                    },
                    "Skin must be at most 50 mm.",
                ),
            ];
            for (form, expected) in cases {
                assert_eq!(parse(form).unwrap_err(), expected, "typed {typed}");
            }
        }
        for typed in ["1000.5", "1e300"] {
            let form = FormFields {
                min_width_mm: typed.to_string(),
                ..fields()
            };
            assert_eq!(
                parse(form).unwrap_err(),
                "Minimum width must be at most 1000 mm.",
                "typed {typed}"
            );
        }
    }

    #[test]
    fn input_messages_are_told_from_run_messages() {
        for message in [
            NO_SIZE_MESSAGE,
            BAD_CUT_FIELD_MESSAGE,
            PICK_MATERIAL_MESSAGE,
            "Kerf must be at most 50 mm.",
            "Skin plus allowance (3 mm a side) leaves nothing of the rough: its smallest side is 6 mm.",
            "The skin allowance leaves nothing of the rough.",
            "Minimum width must be greater than 0 mm.",
            "Weighed carat must be a number in ct.",
            MODEL_CHANGED_MESSAGE,
            "Fit to weight failed: Dimensions must not exceed 2000 mm.",
        ] {
            assert!(is_input_message(message), "{message}");
        }
        for message in [
            "",
            crate::gui::rough_plan::run::ALL_EXCLUDED_MESSAGE,
            "Switch to the local library to plan a rough.",
            "Could not show the results.",
            "Could not start the planner: out of threads",
        ] {
            assert!(!is_input_message(message), "{message}");
        }
    }
}
