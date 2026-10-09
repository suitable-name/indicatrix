//! The render: Live Render and its lighting presets, the lighting a design remembers, and custom
//! materials (the two sliders, and typed coefficients).

#![allow(
    clippy::too_many_lines,
    reason = "a tutorial is its step text; splitting it would only scatter the wording"
)]

use super::{VIEWPORT, VIEWPORT_TABS, event};
use crate::guide::{
    Group, Guide, GuideCategory, GuideStep, StartingState,
    tutorials::viewing_events::{
        COEFFICIENT_MATERIAL_SAVED, CUSTOM_MATERIAL_SAVED, DESIGN_LIGHTING_FORGOTTEN,
        DESIGN_LIGHTING_SAVED, LIGHTING_PRESET_CHOSEN, LIVE_RENDER_OPENED,
    },
};

/// The Design Settings panel (its "New/Edit material..." button) and Undo.
const MATERIALS: &[Group] = &[Group::DesignSettings, Group::History];

/// The lessons of this file, in browser order.
pub fn guides() -> Vec<Guide> {
    vec![
        live_render(),
        design_lighting(),
        custom_material(),
        material_coefficients(),
    ]
}

/// The "open Live Render" step the first two lessons share.
fn open_live_render() -> GuideStep {
    GuideStep::new(
        "Open Live Render",
        "Live Render is the physically based picture of the stone: the light, the material and the dispersion as they really behave.",
    )
    .actions([
        "Click Live Render, the pill to the left of Edit, above the picture.",
    ])
    .check("The traced picture of the stone appears, with its toolbar of Render Material, Lighting and the metrics.")
    .why("The Edit tab's Solid view is quick and flat-shaded. Live Render is slower and shows colour play, so it settles after a moment.")
    .goal(event(LIVE_RENDER_OPENED), "The Live Render view")
    .highlight("live_render_tab")
    .allow(VIEWPORT_TABS)
}

fn live_render() -> Guide {
    Guide::new(
        "viewing-live-render",
        "Live Render and lighting",
        "Look at the stone as light really treats it, and change the lighting preset.",
        GuideCategory::Viewing,
    )
    .starting(StartingState::Template(5))
    .step(open_live_render())
    .step(
        GuideStep::new(
            "Turn the stone and move the light",
            "The picture answers to the mouse as the Solid view does.",
        )
        .actions([
            "Drag to turn the stone, and scroll to zoom.",
            "Right-drag, or Shift and left-drag, moves the light.",
            "Front and Top snap the camera to the standard poses; the reset button next to them puts the camera and the light back.",
        ])
        .why("Turning the stone here turns it in the Solid view too, so the two always show the same pose.")
        .allow(VIEWPORT),
    )
    .step(
        GuideStep::new(
            "Choose a lighting preset",
            "The Lighting drop-down changes the light the stone sits in.",
        )
        .actions([
            "Open the Lighting drop-down in the toolbar.",
            "Choose a different preset, for example Gem Studio Ring Lights or Daylight sky + direct sun.",
        ])
        .check("A toast names the preset, and the picture starts again under the new light.")
        .why("The studio rigs give hard sparkle on a dark backdrop. The lit models (Grading tray, Light tent + black cards, Daylight sky + direct sun) show what the stone looks like in a real scene.")
        .goal(event(LIGHTING_PRESET_CHOSEN), "A lighting preset chosen")
        .highlight("lighting_combo")
        .allow(VIEWPORT),
    )
    .step(
        GuideStep::new(
            "Render Material",
            "The Render Material drop-down changes how the render looks, never the cut.",
        )
        .actions([
            "Next to it, Linked to design makes the render follow the material in the Edit tab's Design Settings.",
            "Picking a material by hand here turns that link off, so the render and the design can differ.",
            "The pencil button opens the Material Editor (see the custom materials tutorial).",
        ])
        .why("To change the material the stone is cut for, use Design Settings in the Edit tab, not this drop-down.")
        .allow(VIEWPORT),
    )
    .step(
        GuideStep::new(
            "Back to the Edit tab",
            "The Edit tab is where you change the design.",
        )
        .actions(["Click Edit, the pill to the right of Live Render."])
        .why("Every control is unlocked again."),
    )
}

fn design_lighting() -> Guide {
    Guide::new(
        "viewing-design-lighting",
        "Lighting for this design",
        "Make a design remember its own lighting, and forget it again.",
        GuideCategory::Viewing,
    )
    .starting(StartingState::Template(5))
    .step(open_live_render())
    .step(
        GuideStep::new(
            "Choose the lighting",
            "A design can remember the lighting you see on it, so it always opens under the light you chose.",
        )
        .actions([
            "Open the Lighting drop-down in the toolbar.",
            "Choose the preset you want this design to keep.",
        ])
        .check("A toast names the preset.")
        .why("Light direction, exposure, surface glare, the backdrop and an environment map belong to the lighting too; the Rendering Settings dialog changes them.")
        .goal(event(LIGHTING_PRESET_CHOSEN), "A lighting preset chosen")
        .highlight("lighting_combo")
        .allow(VIEWPORT),
    )
    .step(
        GuideStep::new(
            "Save it for this design",
            "Saving is one button in the Rendering Settings dialog, which covers this panel while it is open.",
        )
        .actions([
            "Click the Rendering settings button (the gear) in the Live Render toolbar.",
            "Find the group called Lighting for this design.",
            "Click Use this lighting for this design.",
            "Close the dialog with Done, or press Escape, to come back to this panel.",
        ])
        .check("A toast says Saved. This design will open with this lighting.")
        .why("The lighting is kept in your library on this computer, never in the design file. Saving again replaces it.")
        .goal(event(DESIGN_LIGHTING_SAVED), "Lighting saved for this design")
        .allow(VIEWPORT),
    )
    .step(
        GuideStep::new(
            "Forget it again",
            "The same group holds the way back.",
        )
        .actions([
            "Open the Rendering settings dialog again.",
            "In Lighting for this design, click Forget for this design (it appears once lighting is saved).",
            "Close the dialog.",
        ])
        .check("A toast says This design now uses your normal lighting.")
        .goal(event(DESIGN_LIGHTING_FORGOTTEN), "Lighting forgotten for this design")
        .allow(VIEWPORT),
    )
    .step(
        GuideStep::new(
            "Where a design's lighting applies",
            "A remembered lighting comes back when you open the design in the Edit tab.",
        )
        .actions([
            "Opening a design that has no saved lighting brings your normal lighting back.",
            "A design you only browse in the catalogue is previewed under your normal lighting, not its saved one.",
        ])
        .why("Every control is unlocked again."),
    )
}

fn custom_material() -> Guide {
    Guide::new(
        "viewing-custom-material",
        "Custom materials",
        "Define a material of your own with the Material Editor: refractive index, dispersion, double refraction and colour.",
        GuideCategory::Viewing,
    )
    .starting(StartingState::Template(5))
    .step(
        GuideStep::new(
            "What a custom material holds",
            "The built-in list has 33 materials. A custom material is one you describe yourself, for a stone the list does not have.",
        )
        .actions([
            "Refractive index (nd) and dispersion (the fire, as the spread between the blue and red lines).",
            "Double refraction (birefringence), the crystal system and the optical character.",
            "A body colour, and the specific gravity if you know it, which the Rough Planner uses to turn volume into carats.",
            "Load Preset Template starts you from a built-in material to tweak.",
        ])
        .why("A custom material is saved in your library and appears in every material list, after the built-ins.")
        .highlight("design_settings")
        .allow(MATERIALS),
    )
    .step(
        GuideStep::new(
            "Make a material",
            "The Material Editor opens over the window, so this step carries everything you need.",
        )
        .actions([
            "Click the Edit tab, then New/Edit material... in the Design Settings panel.",
            "Optionally pick a Load Preset Template, for example Quartz.",
            "Type a Material Name that no built-in material uses, for example Tutorial glass.",
            "Move the Refractive Index (nd) and Dispersion sliders to values you like.",
            "Click Save. Save & Apply would also use it for the Live Render picture.",
        ])
        .check("A toast says Saved custom material with the name you typed.")
        .why("The name must not be one of the built-in names, because that would hide the built-in material everywhere. Save is refused with a message if it is.")
        .goal(event(CUSTOM_MATERIAL_SAVED), "A custom material saved")
        .highlight("design_settings")
        .allow(MATERIALS),
    )
    .step(
        GuideStep::new(
            "Use it on the design",
            "Saving only adds the material to the lists. The design keeps its own material until you change it.",
        )
        .actions([
            "In Design Settings, open the Material drop-down: your material is listed after the built-ins.",
            "Pick it, then click Apply Material.",
            "To change or remove it later, open the Material Editor, pick it and use Save or Delete.",
        ])
        .why("Every control is unlocked again."),
    )
}

fn material_coefficients() -> Guide {
    Guide::new(
        "viewing-material-coefficients",
        "Materials from coefficients",
        "Type a published Sellmeier or Cauchy fit as a material's refractive index curve.",
        GuideCategory::Viewing,
    )
    .starting(StartingState::Template(5))
    .step(
        GuideStep::new(
            "Sliders or coefficients",
            "Two sliders describe a curve with two numbers: the index at the yellow sodium line and the spread between blue and red. A published fit describes it exactly.",
        )
        .actions([
            "Sellmeier, 3 terms: n squared is 1 plus three terms; fields B1, B2, B3, C1, C2, C3. Glass catalogues print this form.",
            "Sellmeier, 1 term: the same with one term; fields B1 and C1.",
            "Cauchy: n = A + B/λ² + C/λ^4; fields A, B and C.",
            "The wavelength λ is always in micrometres, so 589 nm is 0.589.",
        ])
        .why("The stone is then traced with exactly your curve, on the graphics card and on the processor alike.")
        .highlight("design_settings")
        .allow(MATERIALS),
    )
    .step(
        GuideStep::new(
            "Type a curve",
            "The Material Editor opens over the window, so this step carries everything you need.",
        )
        .actions([
            "Click the Edit tab, then New/Edit material... in the Design Settings panel.",
            "Switch Refractive index curve from Simple to Coefficients.",
            "Choose the Sellmeier, 3 terms model, then pick Glass (N-BK7) in Copy from to fill the six fields.",
            "Change one number a little and watch the readout under the fields: nd, nF, nC, the spread and the Abbe number.",
            "Type a Material Name that no built-in material uses, for example Tutorial coefficients, and click Save.",
        ])
        .check("A toast says Saved custom material with the name you typed.")
        .why("Save stays disabled while a field is not a number or the curve cannot be traced; the red line under the fields says why.")
        .goal(event(COEFFICIENT_MATERIAL_SAVED), "A coefficient material saved")
        .highlight("design_settings")
        .allow(MATERIALS),
    )
    .step(
        GuideStep::new(
            "Errors and warnings",
            "The editor checks the whole curve, not only each field.",
        )
        .actions([
            "A red message means the curve cannot be traced: a resonance (a Sellmeier C) inside 300 to 800 nm, an index at or below 1, or a calculation that divides by zero.",
            "A C is the resonance wavelength squared in µm², so a resonance at 100 nm is C = 0.01. A resonance in the visible range is nearly always a unit slip.",
            "An amber message is a warning and Save still works: an index that rises with wavelength, an Abbe number outside 5 to 120, or no fire at all.",
        ])
        .why("Switching a material back to Simple and saving replaces the curve with a fit of nd and the spread. Every control is unlocked again."),
    )
}
