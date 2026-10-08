//! The Live Render toolbar's "Color" control: a quick colour change that needs no custom
//! material.
//!
//! The control offers "Material default", the same body-colour presets the Edit tab's
//! color box offers, and "Custom color..." (the material editor's colour picker). Where a
//! pick goes depends on what the view shows (see [`ColorTarget`]):
//!
//! * the editor's open design with the render material linked to it: the pick is that
//!   design's own colour override, the same undoable edit the Edit tab's color box makes
//!   (the editor side supplies it as a [`DesignColorPort`]);
//! * anything else (a library design, or the render material unlinked): a view-only
//!   setting, `RenderContext::view_body_color`, remembered in
//!   `AppSettings::render_body_color_override` and never written to a design.
//!
//! The render pipeline applies the setting in one place
//! (`RenderContext::tinted_material_override`), so the live view, high-resolution export,
//! tilt video and remote workers all render the same colour. A material that defines its
//! own colour (a physics recipe) is never recoloured and the control is greyed out.
//!
//! Everything decided here is a plain function ([`control_view`], [`choice_for`],
//! [`pick_for_choice`], [`display_rgb_for_stone`]) so it is tested without a window.

use std::{
    rc::Rc,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
};

use indicatrix::{
    color::body_color::{Illuminant, body_colors, srgb_to_lab},
    optics::{
        absorption::{AbsorptionTensor, legacy_rgb_bands},
        materials::body_color::{BODY_COLOR_PRESETS, preset_index_for_rgb},
    },
};
use indicatrix_cut_core::material::color::{nearest_legacy_rgb, swatch_path_units};
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};

use crate::{
    MainWindow, ViewportModel,
    bridge::render_thread::{ColorTarget, RenderContext},
    gui::show_toast,
    settings::SettingsPersister,
};

/// A body-colour override: the absorption triple `GemMaterial::with_body_color` takes, or
/// `None` for the material's own colour.
pub type ColorOverride = Option<[f32; 3]>;

/// How many presets the control offers (the editor's own table).
const PRESET_COUNT: usize = BODY_COLOR_PRESETS.len();

/// The choice index of "Custom color...": after "Material default" (0) and the presets
/// (`1..=PRESET_COUNT`).
pub const CUSTOM_CHOICE: usize = PRESET_COUNT + 1;

/// Where the colour picker starts while the control shows "Material default".
const PICKER_START: slint::Color = slint::Color::from_rgb_u8(0xc0, 0x39, 0x2b);

/// What a clicked row of the control means.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Pick {
    /// "Material default": the material's own colour.
    MaterialDefault,
    /// One of the presets.
    Preset([f32; 3]),
}

impl Pick {
    /// The override this pick sets.
    #[must_use]
    pub const fn color(self) -> ColorOverride {
        match self {
            Self::MaterialDefault => None,
            Self::Preset(rgb) => Some(rgb),
        }
    }
}

/// The row `index` stands for: 0 is "Material default", `1..=PRESET_COUNT` are the presets
/// in the editor's order. `None` for "Custom color..." (the picker handles that) and for
/// anything out of range.
#[must_use]
pub fn pick_for_choice(index: i32) -> Option<Pick> {
    let index = usize::try_from(index).ok()?;
    match index {
        0 => Some(Pick::MaterialDefault),
        _ => BODY_COLOR_PRESETS
            .get(index - 1)
            .map(|preset| Pick::Preset(preset.absorption_rgb)),
    }
}

/// The row that shows `color`: 0 for no override, the preset's row for an exact preset
/// triple, [`CUSTOM_CHOICE`] for any other colour.
#[must_use]
pub fn choice_for(color: ColorOverride) -> usize {
    color.map_or(0, |rgb| {
        preset_index_for_rgb(rgb).map_or(CUSTOM_CHOICE, |preset| preset + 1)
    })
}

/// The name shown for row `choice`.
#[must_use]
pub fn choice_label(choice: usize) -> &'static str {
    match choice {
        0 => "Material default",
        _ => choice
            .checked_sub(1)
            .and_then(|preset| BODY_COLOR_PRESETS.get(preset))
            .map_or("Custom color", |preset| preset.label),
    }
}

/// The sRGB colour (D65) a stone with this absorption triple shows at `stone_width_mm`
/// (`0.0` = no size set): the swatch path is
/// `MODEL_UNIT_FACE_UP_PATH` times the scale the render gives a per-model-unit
/// colour at that size ([`swatch_path_units`], `width / 7 mm`), so the dot is the colour the
/// face-up render shows (the calibration of `render_setup::MODEL_UNIT_FACE_UP_PATH`) and darkens
/// with the stone like the render does. With no size or 7 mm it is the 1-unit reference. The
/// toolbar's preset dots call this with the current Stone Size.
#[must_use]
pub fn display_rgb_for_stone(absorption: [f32; 3], stone_width_mm: f32) -> [u8; 3] {
    let tensor = AbsorptionTensor::isotropic(legacy_rgb_bands(absorption));
    body_colors(&tensor, swatch_path_units(stone_width_mm), Illuminant::D65)
        .unpolarised
        .srgb
}

/// Everything [`control_view`] decides from.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ControlInput {
    /// Who a pick changes right now.
    pub target: ColorTarget,
    /// The traced material defines its own colour (a physics recipe).
    pub physics: bool,
    /// The open design's colour override, as applied.
    pub design_color: ColorOverride,
    /// The remembered view-only colour.
    pub view_color: ColorOverride,
    /// The stone size the render uses (`RenderContext::stone_width_mm`, `0.0` = not set), which
    /// the swatch is shown at.
    pub stone_width_mm: f32,
}

/// What the control shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ControlView {
    /// The selected row: see [`choice_for`].
    pub choice: usize,
    /// The selected row's name.
    pub summary: &'static str,
    /// The colour the selection renders in; `None` for "Material default".
    pub swatch: Option<[u8; 3]>,
    /// False for a material that defines its own colour.
    pub enabled: bool,
    /// The reason for `enabled == false`.
    pub physics: bool,
    /// A pick edits the open design (true) or only the view (false).
    pub targets_design: bool,
}

/// The control's state: the design's colour while picks go to the design, the view colour
/// while they go to the view (and the material can be recoloured at all).
#[must_use]
pub fn control_view(input: ControlInput) -> ControlView {
    let shown = match (input.target, input.physics) {
        (ColorTarget::Design, _) => input.design_color,
        (ColorTarget::View, false) => input.view_color,
        // A view colour never applies to a material with its own colour: show none.
        (ColorTarget::View, true) => None,
    };
    let choice = choice_for(shown);
    ControlView {
        choice,
        summary: choice_label(choice),
        swatch: shown.map(|rgb| display_rgb_for_stone(rgb, input.stone_width_mm)),
        enabled: !input.physics,
        physics: input.physics,
        targets_design: input.target == ColorTarget::Design,
    }
}

/// Applies `change` to the render context and marks the render dirty only when the colour
/// the render pipeline applies actually moved (a dormant view colour is not a scene change).
fn update_context(ctx: &mut RenderContext, change: impl FnOnce(&mut RenderContext)) {
    let before = ctx.effective_view_body_color();
    change(ctx);
    if ctx.effective_view_body_color() != before {
        ctx.dirty = true;
    }
}

/// What the editor side offers the control: reading and editing the open design's colour
/// override. Kept behind closures so this module needs nothing from the editor.
pub struct DesignColorPort {
    /// The open design's colour override as applied. The outer `None` means it cannot be
    /// read right now (the editor state is busy); the control then keeps what it shows.
    pub current: Box<dyn Fn() -> Option<ColorOverride>>,
    /// Sets the open design's colour override as ONE undoable edit (the Edit tab's color
    /// box does the same), or says why it could not.
    pub apply: Box<dyn Fn(ColorOverride) -> Result<(), String>>,
}

/// The preset dots' colours at a stone of `stone_width_mm` (`0.0` = no size set): the same
/// path as the selected swatch ([`display_rgb_for_stone`]), so a dot and the swatch it becomes
/// when picked agree at every Stone Size.
fn preset_swatch_colors(stone_width_mm: f32) -> Vec<slint::Color> {
    BODY_COLOR_PRESETS
        .iter()
        .map(|preset| {
            let [r, g, b] = display_rgb_for_stone(preset.absorption_rgb, stone_width_mm);
            slint::Color::from_rgb_u8(r, g, b)
        })
        .collect()
}

/// Pushes the preset dots' colours for the current stone size (they follow Stone Size).
fn push_preset_swatches(ui: &MainWindow, stone_width_mm: f32) {
    ui.global::<ViewportModel>()
        .set_render_color_preset_swatches(ModelRc::new(VecModel::from(preset_swatch_colors(
            stone_width_mm,
        ))));
}

/// Pushes the 9 preset names (they never change) and the dots' colours at `stone_width_mm`.
fn push_presets(ui: &MainWindow, stone_width_mm: f32) {
    let labels: Vec<SharedString> = BODY_COLOR_PRESETS
        .iter()
        .map(|preset| SharedString::from(preset.label))
        .collect();
    let model = ui.global::<ViewportModel>();
    model.set_render_color_preset_labels(ModelRc::new(VecModel::from(labels)));
    push_preset_swatches(ui, stone_width_mm);
    model.set_render_color_custom_index(i32::try_from(CUSTOM_CHOICE).unwrap_or(i32::MAX));
}

/// Pushes `view` into the Slint model.
fn push_view(ui: &MainWindow, view: &ControlView) {
    let model = ui.global::<ViewportModel>();
    model.set_render_color_index(i32::try_from(view.choice).unwrap_or(0));
    model.set_render_color_summary(view.summary.into());
    model.set_render_color_has_swatch(view.swatch.is_some());
    model.set_render_color_current(
        view.swatch
            .map_or(PICKER_START, |[r, g, b]| slint::Color::from_rgb_u8(r, g, b)),
    );
    model.set_render_color_enabled(view.enabled);
    model.set_render_color_physics(view.physics);
    model.set_render_color_targets_design(view.targets_design);
}

/// Recomputes the control from the render context and the open design.
fn refresh(ui: &MainWindow, render_ctx: &Arc<Mutex<RenderContext>>, port: &DesignColorPort) {
    let (target, physics, view_color, stone_width_mm) = {
        let ctx = RenderContext::lock(render_ctx);
        (
            ctx.color_target(),
            ctx.physics_color(),
            ctx.view_body_color,
            ctx.stone_width_mm,
        )
    };
    push_preset_swatches(ui, stone_width_mm);
    let design_color = if target == ColorTarget::Design {
        let Some(current) = (port.current)() else {
            return;
        };
        current
    } else {
        None
    };
    push_view(
        ui,
        &control_view(ControlInput {
            target,
            physics,
            design_color,
            view_color,
            stone_width_mm,
        }),
    );
}

/// Applies a pick where it belongs right now: the open design (one undo step) or the view
/// setting (remembered), then refreshes the control.
fn apply_pick(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    port: &DesignColorPort,
    color: ColorOverride,
) {
    let (target, physics) = {
        let ctx = RenderContext::lock(render_ctx);
        (ctx.color_target(), ctx.physics_color())
    };
    if physics {
        // The control is greyed out for these; never recolour a material's own colour.
        return;
    }
    match target {
        ColorTarget::Design => {
            if let Err(message) = (port.apply)(color) {
                show_toast(ui, &message, "error");
            }
        }
        ColorTarget::View => {
            update_context(&mut RenderContext::lock(render_ctx), |ctx| {
                ctx.view_body_color = color;
            });
            if let Some(persister) = SettingsPersister::installed_for_this_thread() {
                persister.update(|file| file.settings.render_body_color_override = color);
            }
        }
    }
    refresh(ui, render_ctx, port);
}

/// Wires the toolbar's "Color" control: pushes the presets, restores the remembered view
/// colour, and registers the model's callbacks. `port` is the editor's side of the design
/// colour (see [`DesignColorPort`]).
pub fn setup_render_body_color_callbacks(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    port: DesignColorPort,
) {
    let port = Rc::new(port);
    push_presets(ui, RenderContext::lock(render_ctx).stone_width_mm);

    // Restore the remembered view colour and the link state the context mirrors.
    let saved = SettingsPersister::installed_for_this_thread()
        .and_then(|persister| persister.snapshot().settings.render_body_color_override);
    let linked = ui.global::<ViewportModel>().get_viewport_material_linked();
    update_context(&mut RenderContext::lock(render_ctx), |ctx| {
        ctx.view_body_color = saved;
        ctx.material_linked = linked;
    });

    let model = ui.global::<ViewportModel>();

    let (ui_weak, ctx, port_refresh) = (ui.as_weak(), Arc::clone(render_ctx), Rc::clone(&port));
    model.on_render_color_refresh(move || {
        if let Some(ui) = ui_weak.upgrade() {
            refresh(&ui, &ctx, &port_refresh);
        }
    });

    // "Linked to design" moved: the render pipeline mirrors it (it decides whether the
    // view colour or the design's own colour applies), then the control follows.
    let (ui_weak, ctx, port_link) = (ui.as_weak(), Arc::clone(render_ctx), Rc::clone(&port));
    model.on_render_color_context_changed(move || {
        let Some(ui) = ui_weak.upgrade() else {
            return;
        };
        let linked = ui.global::<ViewportModel>().get_viewport_material_linked();
        update_context(&mut RenderContext::lock(&ctx), |c| {
            c.material_linked = linked;
        });
        refresh(&ui, &ctx, &port_link);
    });

    let (ui_weak, ctx, port_pick) = (ui.as_weak(), Arc::clone(render_ctx), Rc::clone(&port));
    model.on_render_color_chosen(move |index| {
        let (Some(ui), Some(pick)) = (ui_weak.upgrade(), pick_for_choice(index)) else {
            return;
        };
        apply_pick(&ui, &ctx, &port_pick, pick.color());
    });

    // "Custom color...": the picked screen colour is matched to the closest colour the
    // material model can reach on a background thread (the material editor's colour picker
    // does the same), then applied like any other pick. A newer pick supersedes an older
    // one that is still being matched.
    let latest = Arc::new(AtomicU64::new(0));
    let ui_weak = ui.as_weak();
    model.on_render_color_custom_picked(move |picked| {
        let Some(ui) = ui_weak.upgrade() else {
            return;
        };
        let ticket = latest.fetch_add(1, Ordering::SeqCst) + 1;
        let target_lab = srgb_to_lab([
            f64::from(picked.red()) / 255.0,
            f64::from(picked.green()) / 255.0,
            f64::from(picked.blue()) / 255.0,
        ]);
        ui.global::<ViewportModel>().set_render_color_busy(true);
        let (latest, ui_weak) = (Arc::clone(&latest), ui.as_weak());
        std::thread::spawn(move || {
            let [r, g, b] = nearest_legacy_rgb(target_lab);
            let _ = ui_weak.upgrade_in_event_loop(move |ui| {
                if latest.load(Ordering::SeqCst) == ticket {
                    ui.global::<ViewportModel>()
                        .invoke_render_color_custom_solved(r, g, b);
                }
            });
        });
    });

    let (ui_weak, ctx, port_solved) = (ui.as_weak(), Arc::clone(render_ctx), Rc::clone(&port));
    model.on_render_color_custom_solved(move |r, g, b| {
        let Some(ui) = ui_weak.upgrade() else {
            return;
        };
        ui.global::<ViewportModel>().set_render_color_busy(false);
        apply_pick(&ui, &ctx, &port_solved, Some([r, g, b]));
    });

    refresh(ui, render_ctx, &port);
}

#[cfg(test)]
mod tests {
    use super::*;

    const YELLOW: [f32; 3] = BODY_COLOR_PRESETS[5].absorption_rgb;

    fn input(target: ColorTarget, physics: bool) -> ControlInput {
        ControlInput {
            target,
            physics,
            design_color: Some(BODY_COLOR_PRESETS[1].absorption_rgb),
            view_color: Some(YELLOW),
            stone_width_mm: 0.0,
        }
    }

    #[test]
    fn rows_map_to_the_same_presets_the_edit_tab_offers() {
        assert_eq!(pick_for_choice(0), Some(Pick::MaterialDefault));
        for (index, preset) in BODY_COLOR_PRESETS.iter().enumerate() {
            let row = i32::try_from(index + 1).expect("small");
            assert_eq!(
                pick_for_choice(row),
                Some(Pick::Preset(preset.absorption_rgb)),
                "row {row}"
            );
            // The same index the editor's own combo uses (`body_color_index_for`).
            assert_eq!(
                choice_for(Some(preset.absorption_rgb)),
                index + 1,
                "{}",
                preset.label
            );
            assert_eq!(choice_label(index + 1), preset.label);
        }
    }

    #[test]
    fn the_custom_row_and_out_of_range_rows_are_not_picks() {
        let custom = i32::try_from(CUSTOM_CHOICE).expect("small");
        assert_eq!(pick_for_choice(custom), None);
        assert_eq!(pick_for_choice(custom + 5), None);
        assert_eq!(pick_for_choice(-1), None);
    }

    #[test]
    fn a_pick_carries_exactly_the_override_it_stands_for() {
        assert_eq!(Pick::MaterialDefault.color(), None);
        assert_eq!(Pick::Preset(YELLOW).color(), Some(YELLOW));
    }

    #[test]
    fn no_colour_is_the_default_row_and_an_unlisted_colour_is_the_custom_row() {
        assert_eq!(choice_for(None), 0);
        assert_eq!(choice_label(0), "Material default");
        assert_eq!(choice_for(Some([0.5, 0.5, 0.5])), CUSTOM_CHOICE);
        assert_eq!(choice_label(CUSTOM_CHOICE), "Custom color");
        assert_eq!(choice_label(CUSTOM_CHOICE + 3), "Custom color");
    }

    #[test]
    fn the_control_shows_the_designs_colour_while_picks_go_to_the_design() {
        let view = control_view(input(ColorTarget::Design, false));
        assert_eq!(view.choice, 2, "Blue is the second preset");
        assert_eq!(view.summary, "Blue");
        assert!(view.targets_design);
        assert!(view.enabled);
        assert!(view.swatch.is_some());
    }

    #[test]
    fn the_control_shows_the_view_colour_while_picks_only_change_the_view() {
        let view = control_view(input(ColorTarget::View, false));
        assert_eq!(view.choice, 6, "Yellow is the sixth preset");
        assert_eq!(view.summary, "Yellow");
        assert!(!view.targets_design);
        assert!(view.enabled);
    }

    #[test]
    fn material_default_has_no_swatch_and_a_custom_colour_does() {
        let mut state = input(ColorTarget::View, false);
        state.view_color = None;
        let view = control_view(state);
        assert_eq!((view.choice, view.swatch), (0, None));

        state.view_color = Some([0.3, 0.9, 1.4]);
        let view = control_view(state);
        assert_eq!(view.choice, CUSTOM_CHOICE);
        assert_eq!(view.summary, "Custom color");
        assert!(view.swatch.is_some());
    }

    #[test]
    fn a_physics_material_greys_the_control_out_and_never_shows_a_view_colour() {
        let view = control_view(input(ColorTarget::View, true));
        assert!(!view.enabled);
        assert!(view.physics);
        assert_eq!(
            (view.choice, view.swatch),
            (0, None),
            "the remembered view colour does not apply to this material"
        );
        // A design that already carries an override on a physics material still shows it
        // (the editor warns about it the same way); the control stays greyed out.
        let view = control_view(input(ColorTarget::Design, true));
        assert!(!view.enabled);
        assert_eq!(view.choice, 2);
    }

    #[test]
    fn the_swatch_colours_look_like_their_names() {
        let [cr, cg, cb] = display_rgb_for_stone(BODY_COLOR_PRESETS[0].absorption_rgb, 0.0);
        assert!(
            cr > 200 && cg > 200 && cb > 200,
            "Clear is near white: {cr} {cg} {cb}"
        );
        let [r, _, b] = display_rgb_for_stone(BODY_COLOR_PRESETS[1].absorption_rgb, 0.0);
        assert!(b > r, "Blue is bluer than it is red: r={r} b={b}");
        let [r, g, b] = display_rgb_for_stone(BODY_COLOR_PRESETS[5].absorption_rgb, 0.0);
        assert!(r > b && g > b, "Yellow is warm: {r} {g} {b}");
        let [r, g, b] = display_rgb_for_stone(BODY_COLOR_PRESETS[2].absorption_rgb, 0.0);
        assert!(r > g && r > b, "Red is red: {r} {g} {b}");
    }

    /// The real swatch path (`display_rgb_for_stone`, via `swatch_path_units`) against the
    /// render's own rule (`material_for_stone` scale times `MODEL_UNIT_FACE_UP_PATH`), at 5, 7
    /// and 10.87 mm, for every preset: the dot is the colour the render shows. D65 on both sides,
    /// 1/255 per channel.
    #[test]
    fn the_desktop_swatch_equals_the_sized_render_material_at_every_size() {
        use indicatrix::{
            optics::materials::GemMaterial,
            render_setup::{MODEL_UNIT_FACE_UP_PATH, material_for_stone},
        };
        for preset in BODY_COLOR_PRESETS.iter() {
            let material = GemMaterial::by_name("Cubic Zirconia")
                .expect("built-in")
                .with_body_color(preset.absorption_rgb);
            for width_mm in [5.0_f32, 7.0, 10.87] {
                let stone = material_for_stone(material.clone(), width_mm, &[]);
                let render = body_colors(
                    &stone.absorption,
                    f64::from(MODEL_UNIT_FACE_UP_PATH) * f64::from(stone.absorption_path_scale),
                    Illuminant::D65,
                )
                .unpolarised
                .srgb;
                let swatch = display_rgb_for_stone(preset.absorption_rgb, width_mm);
                for (r, s) in render.iter().zip(swatch) {
                    assert!(
                        r.abs_diff(s) <= 1,
                        "{} at {width_mm} mm: render {render:?} vs swatch {swatch:?}",
                        preset.label
                    );
                }
            }
        }
    }

    #[test]
    fn picking_a_view_colour_marks_the_render_dirty_only_when_it_applies() {
        let mut ctx = RenderContext {
            material_linked: false,
            dirty: false,
            ..Default::default()
        };
        update_context(&mut ctx, |c| c.view_body_color = Some(YELLOW));
        assert!(ctx.dirty, "a new colour on a free view restarts the render");
        assert_eq!(ctx.view_body_color, Some(YELLOW));

        // The same colour again changes nothing.
        ctx.dirty = false;
        update_context(&mut ctx, |c| c.view_body_color = Some(YELLOW));
        assert!(!ctx.dirty);

        // While the linked open design drives the colour the setting is dormant: changing
        // it, or the link, only restarts the render when what is shown really changes.
        ctx.planes_owner = crate::bridge::render_thread::PlanesOwner::Editor { generation: 1 };
        update_context(&mut ctx, |c| c.material_linked = true);
        assert!(ctx.dirty, "the view colour stopped applying");
        ctx.dirty = false;
        update_context(&mut ctx, |c| c.view_body_color = None);
        assert!(!ctx.dirty, "a dormant colour is not a scene change");
    }
}
