//! The worked-example walkthrough's static step list. Every step's title/body
//! is derived from `docs/manual/07-new-design-worked-example.md`; `highlight_target` names which
//! owned component should draw its own 2px accent outline while that step is
//! current (`"tier_table"` for `EditorTierTable`, `"design_settings"` for
//! `EditorDesignSettings`, `"new_design_dialog"` for `NewDesignDialog`), or
//! `""` for a step whose subject lives elsewhere (e.g., in the Inspector) and
//! says so in its own body text instead. See [`ui::models::guide::GuideModel`]
//! (`ui/models/guide.slint`) for the Slint side: navigation (`next`/`back`/`close`)
//! is implemented entirely there, so this module's only job is handing it the
//! step list once at startup.

use crate::{GuideModel, GuideStepData, MainWindow};
use slint::{ComponentHandle as _, ModelRc, SharedString, VecModel};

/// One step's raw text, before being turned into a [`GuideStepData`] -- kept
/// as a plain `const` tuple array (not `GuideStepData` literals directly)
/// since a Slint-generated struct has no `const fn` constructor.
const STEPS: &[(&str, &str, &str)] = &[
    (
        "Start a new design",
        "Open the New Design dialog (the empty state's \"New from Template\" card, \
         or File > New Design...). For this walkthrough: Preform Shape Cylinder, \
         Half-Width 1.50, Length/Width 1.00, Depth 1.50, Index Gear 96, Symmetry \
         Order 8, Mirror on, Starting Material (none). Click Create.",
        "new_design_dialog",
    ),
    (
        "Add the girdle first",
        "The girdle needs to exist before the crown and pavilion, since it gives \
         both blocks something to meet, and it needs its own anchor. On the Tier \
         tab: Angle 90.0 (exactly -- 0.0 would classify as a second table, not a \
         girdle), Meets: Exact scale value 1.0, Name G1, Indices \
         0,12,24,36,48,60,72,84. Click Add Tier, then check the tier table's own \
         C/P/G column reads G for this row.",
        "tier_table",
    ),
    (
        "Add the pavilion main facets",
        "Eight facets, angled steeply below the girdle: Angle -40.0 (a typical \
         starting point -- refine later with Solve and Optimize), Meets: Named \
         facet(s) G1, Name P1, the same eight indices as the girdle. Click Add \
         Tier, then check the MARGIN column reads comfortably Safe.",
        "tier_table",
    ),
    (
        "Add the crown main facets",
        "Eight facets, angled above the girdle: Angle 34.5, Meets: Named facet(s) \
         G1, Name C1, the same eight indices again. Click Add Tier.",
        "tier_table",
    ),
    (
        "Add the table",
        "One flat facet at the top, not indexed around the gear: Angle 0.0, \
         Meets: Unspecified vertex (or Exact scale value, if you want to state the \
         table size directly), Name T, Indices blank. Click Add Tier.",
        "tier_table",
    ),
    (
        "Pick a real material",
        "In the Design Settings panel, set Material to Diamond and click Apply \
         Material. Watch the Effective RI / Critical Angle readouts update, and \
         re-check the pavilion tier's MARGIN column at the new, smaller critical \
         angle.",
        "design_settings",
    ),
    (
        "Solve and check",
        "Click Solve on the command bar (in the Inspector's own command bar row, \
         above the tier table). \"Closed solid -- volume ...\" means you have a \
         valid stone; a \"no anchor\" or \"Degenerate\"/\"Unbounded\" message names \
         the tier most likely responsible -- check its angle and constraint.",
        "",
    ),
    (
        "Check the orbits",
        "For each multi-index tier (girdle, pavilion mains, crown mains), check \
         the ORBIT column reads \"orbit x8\". An amber \"6/8 orbit\" means an index \
         position is missing or inconsistent -- compare the tier's Indices field \
         against the intended list.",
        "tier_table",
    ),
    (
        "Yield and rendering (optional)",
        "On the Preform tab's Yield section (in the Inspector), set Girdle \
         Diameter (mm) to a real size and pick a Yield Material, then Solve again \
         to see Volumetric Yield and Est. Carat Weight. Switch to the Live Render \
         tab to see the stone rendered in the material you picked.",
        "",
    ),
    (
        "You have a working design",
        "That is the full worked example -- a girdle, pavilion, crown and table \
         that solves and closes. From here: Chapter 8 covers Deep Solve, Optimize, \
         Adopt and Apply for verifying and improving it further.",
        "",
    ),
];

/// Pushes [`STEPS`] into `GuideModel.steps` once, at startup -- called from
/// `gui::editor::setup_editor_callbacks`. `GuideModel`'s own navigation
/// (`next`/`back`/`close`/`start`) needs no Rust callback registration at all
/// (see that global's own doc comment, `ui/models/guide.slint`); this is the
/// entire Rust-side footprint of the guide feature.
pub(in crate::gui::editor) fn setup_guide(ui: &MainWindow) {
    let steps: Vec<GuideStepData> = STEPS
        .iter()
        .map(|(title, body, highlight_target)| GuideStepData {
            title: SharedString::from(*title),
            body: SharedString::from(*body),
            highlight_target: SharedString::from(*highlight_target),
        })
        .collect();
    ui.global::<GuideModel>()
        .set_steps(ModelRc::new(VecModel::from(steps)));
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The guide must actually have steps, and `highlight_target` must only
    /// ever be one of the recognized values (an owned component's own check,
    /// or `""`) -- a typo here would silently mean "no highlight" for a step
    /// that meant to name a real target.
    #[test]
    fn steps_are_non_empty_and_use_recognized_highlight_targets() {
        // `STEPS` is a `const` array literal declared above -- `.is_empty()` on
        // it is compile-time-decidable (clippy::const_is_empty), so the real
        // check below (every step's own fields) is what actually earns its
        // keep.
        const RECOGNIZED: &[&str] = &["", "tier_table", "design_settings", "new_design_dialog"];
        for (title, body, highlight_target) in STEPS {
            assert_ne!(*title, "");
            assert_ne!(*body, "");
            assert!(
                RECOGNIZED.contains(highlight_target),
                "step {title:?} has an unrecognized highlight_target {highlight_target:?}"
            );
        }
    }

    /// Step order must read as a monotonically increasing walkthrough -- this
    /// test is a placeholder for "steps stay in the order this module
    /// declares them" (a plain `const` array's order IS the display order, so
    /// this mostly documents that invariant rather than testing arithmetic on
    /// it), and guards against an accidental duplicate title, which would
    /// make two steps indistinguishable in the panel's own "Step N of M"
    /// counter.
    #[test]
    fn step_titles_are_unique() {
        for (i, (title, ..)) in STEPS.iter().enumerate() {
            for (other, ..) in &STEPS[i + 1..] {
                assert_ne!(title, other, "duplicate guide step title {title:?}");
            }
        }
    }
}
