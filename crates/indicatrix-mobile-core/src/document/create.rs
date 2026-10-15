//! Creating designs: the template cards of the New Design page, a design from a template
//! with its own gear and material, and a blank design from the Empty form.
//!
//! A port of the web app's `editor::new_design` over `indicatrix_editor::templates`.
//! Every function replaces the current design like [`super::open_picked`] does (the
//! caller asks about unsaved changes first) and leaves it unchanged on an error.

use std::sync::OnceLock;

use indicatrix_cut_core::{FreshDesignSpec, MaterialSelection, PreformSpec, templates::TEMPLATES};
use indicatrix_editor::{
    EditorSession,
    material::builtin_preset_names,
    optimize_view::facet_count_from_solved,
    templates::{NewDesignChoice, create_from_template, gallery_design},
};

use super::{DocumentOrigin, DocumentState, install};
use crate::app_state::AppState;

/// The half-width of the rough a blank design starts on (the same generous cylinder the
/// start-up design uses).
pub const DEFAULT_HALF_WIDTH: f64 = 1.5;
/// The depth of the rough a blank design starts on.
pub const DEFAULT_DEPTH: f64 = 1.5;
/// The polygon side count of a round or oval rough: fixed, independent of the gear (the
/// desktop's `FIXED_CYLINDER_PREFORM_SIDES`).
const CYLINDER_SIDES: usize = 96;

/// One card of the New Design page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TemplateCard {
    /// The session template index `EditorSession::from_template` takes: `0` is the Empty
    /// card, `n >= 1` is `TEMPLATES[n - 1]`.
    pub index: usize,
    /// The card's title.
    pub name: String,
    /// The one-line shape and fold description under the title.
    pub description: String,
    /// The full sentence describing the design (for accessibility and the info sheet).
    pub detail: String,
    /// The number of facets of the finished stone; `0` for Empty.
    pub facets: usize,
}

/// The cards, built once: facet counts need a solve per template.
static CARDS: OnceLock<Vec<TemplateCard>> = OnceLock::new();

/// Builds the cards on a background thread so [`template_cards`] is ready by the time
/// the New Design page opens.
///
/// Safe to call more than once; a failure to start the thread
/// only means the first [`template_cards`] call does the work itself.
pub fn warm_template_cards() {
    if CARDS.get().is_some() {
        return;
    }
    let spawned = std::thread::Builder::new()
        .name("template-cards".to_owned())
        .spawn(|| {
            let _ = template_cards();
        });
    if let Err(error) = spawned {
        tracing::debug!(%error, "template cards are built on demand");
    }
}

/// The cards in display order: Empty first, then the nine templates in table order.
#[must_use]
pub fn template_cards() -> Vec<TemplateCard> {
    CARDS.get_or_init(build_cards).clone()
}

fn build_cards() -> Vec<TemplateCard> {
    let mut cards = vec![TemplateCard {
        index: 0,
        name: "Empty Design".to_owned(),
        description: "Blank preform".to_owned(),
        detail: "No starting tiers -- author the schedule from scratch.".to_owned(),
        facets: 0,
    }];
    for (position, spec) in TEMPLATES.iter().enumerate() {
        let index = position + 1;
        let design = gallery_design(i32::try_from(index).unwrap_or(i32::MAX));
        let facets = design
            .solve()
            .ok()
            .map_or(0, |solved| facet_count_from_solved(&design, &solved));
        cards.push(TemplateCard {
            index,
            name: spec.name.to_owned(),
            description: spec.shape.to_owned(),
            detail: spec.description.to_owned(),
            facets,
        });
    }
    cards
}

/// The rough's outline on the Empty form.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PreformShapeChoice {
    /// A cylinder, length equals width.
    #[default]
    Round,
    /// A cylinder, longer than wide.
    Oval,
    /// A block, length equals width.
    Square,
    /// A block, longer than wide.
    Rectangle,
}

impl PreformShapeChoice {
    /// Every choice in the order the form lists them.
    pub const ALL: [Self; 4] = [Self::Round, Self::Oval, Self::Square, Self::Rectangle];

    /// The position in [`Self::ALL`] (a Slint int).
    #[must_use]
    pub const fn index(self) -> i32 {
        match self {
            Self::Round => 0,
            Self::Oval => 1,
            Self::Square => 2,
            Self::Rectangle => 3,
        }
    }

    /// The choice at `index` in [`Self::ALL`].
    #[must_use]
    pub const fn from_index(index: i32) -> Option<Self> {
        match index {
            0 => Some(Self::Round),
            1 => Some(Self::Oval),
            2 => Some(Self::Square),
            3 => Some(Self::Rectangle),
            _ => None,
        }
    }

    /// The label on the form.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Round => "Round",
            Self::Oval => "Oval",
            Self::Square => "Square",
            Self::Rectangle => "Rectangle",
        }
    }

    /// Whether the length-over-width ratio is the user's to set (an oval or a
    /// rectangle); a round or square rough is always 1.
    #[must_use]
    pub const fn has_ratio(self) -> bool {
        matches!(self, Self::Oval | Self::Rectangle)
    }
}

/// The Empty form: every field of a blank design.
#[derive(Debug, Clone, PartialEq)]
pub struct NewDesignForm {
    /// The rough's outline.
    pub shape: PreformShapeChoice,
    /// Half the rough's width in model units.
    pub half_width: f64,
    /// Length over width; ignored (1) for a round or square rough.
    pub length_over_width: f64,
    /// The rough's total depth in model units.
    pub depth: f64,
    /// The index gear's tooth count.
    pub gear_teeth: i32,
    /// The rotational symmetry order; it must divide the gear.
    pub symmetry_order: u32,
    /// Whether the schedule is mirrored.
    pub mirror: bool,
    /// The starting material, a built-in's name; `None` for "(none)".
    pub material_name: Option<String>,
    /// The finished stone's width in millimetres, if known.
    pub width_mm: Option<f64>,
}

impl Default for NewDesignForm {
    fn default() -> Self {
        Self {
            shape: PreformShapeChoice::Round,
            half_width: DEFAULT_HALF_WIDTH,
            length_over_width: 1.0,
            depth: DEFAULT_DEPTH,
            gear_teeth: 96,
            symmetry_order: 8,
            mirror: true,
            material_name: None,
            width_mm: None,
        }
    }
}

/// The Empty form's values exactly as the UI holds them: indices into its lists and the
/// text of its two number fields.
#[derive(Debug, Clone, Copy)]
pub struct RawNewDesignForm<'a> {
    /// Index into [`PreformShapeChoice::ALL`].
    pub shape_index: i32,
    /// The length-over-width text; read only for an oval or a rectangle.
    pub ratio: &'a str,
    /// The index gear's tooth count.
    pub gear_teeth: i32,
    /// The rotational symmetry order.
    pub symmetry: i32,
    /// Whether the schedule is mirrored.
    pub mirror: bool,
    /// Index into [`builtin_preset_names`] (`0` is "(none)").
    pub material_index: i32,
    /// The stone width text in millimetres; blank when unknown.
    pub width_mm: &'a str,
}

/// A positive finite number from `text`; a decimal comma is read as a point (a phone in
/// many locales types one).
fn parse_decimal(text: &str, label: &str) -> Result<f64, String> {
    let cleaned = text.trim().replace(',', ".");
    let value: f64 = cleaned
        .parse()
        .map_err(|_| format!("{label} '{}' is not a number.", text.trim()))?;
    if value.is_finite() && value > 0.0 {
        Ok(value)
    } else {
        Err(format!("{label} must be a positive number."))
    }
}

impl NewDesignForm {
    /// The form for the raw values of the UI, with the rough's size at its defaults.
    ///
    /// # Errors
    /// A sentence naming the field that is not a usable number or choice.
    pub fn from_raw(raw: &RawNewDesignForm<'_>) -> Result<Self, String> {
        let shape = PreformShapeChoice::from_index(raw.shape_index)
            .ok_or_else(|| "Pick a shape.".to_owned())?;
        let length_over_width = if shape.has_ratio() {
            parse_decimal(raw.ratio, "Length over width")?
        } else {
            1.0
        };
        let width_mm = if raw.width_mm.trim().is_empty() {
            None
        } else {
            Some(parse_decimal(raw.width_mm, "Stone width")?)
        };
        let symmetry_order = u32::try_from(raw.symmetry)
            .map_err(|_| "The symmetry order must be a positive whole number.".to_owned())?;
        let names = builtin_preset_names();
        let material_name = usize::try_from(raw.material_index)
            .ok()
            .and_then(|position| names.get(position))
            .filter(|name| name.as_str() != "(none)")
            .cloned();
        Ok(Self {
            shape,
            length_over_width,
            gear_teeth: raw.gear_teeth,
            symmetry_order,
            mirror: raw.mirror,
            material_name,
            width_mm,
            ..Self::default()
        })
    }

    /// The spec the fields describe.
    ///
    /// # Errors
    /// A sentence naming the field that is wrong: a size that is not a positive finite
    /// number, a gear or symmetry that is not a positive whole number, a symmetry that does
    /// not divide the gear, a material that is not a built-in.
    pub fn to_spec(&self) -> Result<FreshDesignSpec, String> {
        let positive = |value: f64| value.is_finite() && value > 0.0;
        if !positive(self.half_width) || !positive(self.depth) {
            return Err("The rough's width and depth must be positive numbers.".to_owned());
        }
        let ratio = if self.shape.has_ratio() {
            self.length_over_width
        } else {
            1.0
        };
        if !positive(ratio) {
            return Err("Length over width must be a positive number.".to_owned());
        }
        if self.gear_teeth <= 0 {
            return Err("The gear's tooth count must be a positive whole number.".to_owned());
        }
        if self.symmetry_order < 1 {
            return Err("The symmetry order must be a positive whole number.".to_owned());
        }
        let teeth = u32::try_from(self.gear_teeth).unwrap_or(0);
        if !teeth.is_multiple_of(self.symmetry_order) {
            return Err(format!(
                "{}-fold symmetry does not divide a {}-tooth gear. Pick a symmetry that does.",
                self.symmetry_order, self.gear_teeth
            ));
        }
        if let Some(name) = &self.material_name
            && !builtin_preset_names().contains(name)
        {
            return Err(format!("'{name}' is not a built-in material."));
        }
        let preform = match self.shape {
            PreformShapeChoice::Round | PreformShapeChoice::Oval => {
                PreformSpec::cylinder(CYLINDER_SIDES, self.half_width, ratio, self.depth)
            }
            PreformShapeChoice::Square | PreformShapeChoice::Rectangle => {
                PreformSpec::block(self.half_width, ratio, self.depth)
            }
        };
        Ok(FreshDesignSpec {
            gear_teeth: self.gear_teeth,
            symmetry_order: self.symmetry_order,
            mirror: self.mirror,
            material: MaterialSelection {
                name: self.material_name.clone(),
                ..MaterialSelection::none()
            },
            preform,
        })
    }
}

/// Installs a blank design from `spec`.
pub fn new_from_spec(state: &mut AppState, spec: FreshDesignSpec) {
    install_new(state, EditorSession::from_spec(spec), DocumentOrigin::New);
}

/// Installs a blank design from the Empty form, with the stone width as the design's
/// girdle diameter.
///
/// # Errors
/// [`NewDesignForm::to_spec`]'s sentence; the current design is unchanged then.
pub fn new_from_form(state: &mut AppState, form: &NewDesignForm) -> Result<(), String> {
    let spec = form.to_spec()?;
    let mut session = EditorSession::from_spec(spec);
    session.design.girdle_diameter_mm = form.width_mm.filter(|mm| mm.is_finite() && *mm > 0.0);
    install_new(state, session, DocumentOrigin::New);
    Ok(())
}

/// Installs the design of template card `index` (`0` is a blank design on the default
/// rough) on the template's own gear and starting material.
///
/// The design's first header
/// line is the template's name, so the page and a save have a title.
///
/// # Errors
/// A sentence when `index` names no template; the current design is unchanged then.
pub fn new_from_template(state: &mut AppState, index: usize) -> Result<(), String> {
    if index == 0 {
        let spec = NewDesignForm::default().to_spec()?;
        install_new(
            state,
            EditorSession::from_spec(spec),
            DocumentOrigin::Template { index },
        );
        return Ok(());
    }
    let spec = TEMPLATES
        .get(index - 1)
        .ok_or_else(|| format!("There is no template number {index} to start from."))?;
    let created = create_from_template(&NewDesignChoice {
        template_index: i32::try_from(index).unwrap_or(i32::MAX),
        gear_teeth: spec.gear_teeth,
        material: spec.default_material_selection(),
        girdle_diameter_mm: None,
        custom_materials: Vec::new(),
    })?;
    let mut session = created.session;
    session.design.meta.headers = vec![spec.name.to_owned()];
    install_new(state, session, DocumentOrigin::Template { index });
    Ok(())
}

/// Installs `session` as a design nobody has saved.
fn install_new(state: &mut AppState, session: EditorSession, origin: DocumentOrigin) {
    let document = DocumentState {
        origin,
        ..DocumentState::default()
    };
    install(state, session, document, None);
    // `install` marks the replacement saved; a design the user created has no file yet.
    state.session.mark_unsaved();
}
