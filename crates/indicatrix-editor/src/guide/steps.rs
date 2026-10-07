//! The worked-example walkthrough's content: ten [`Step`]s, in display order,
//! written against `docs/manual/07-new-design-worked-example.md` (keep the two in
//! step -- same numbers, same control names).
//!
//! Plain `const` data rather than `GuideStepData` literals, since a
//! Slint-generated struct has no `const fn` constructor; the desktop's `setup_guide`
//! maps it once at startup.

/// One group of controls the guide can lock.
///
/// Each variant is one field of `ui/models/guide.slint`'s `GuideAllow` (see that struct's
/// field comments for exactly which controls and keyboard shortcuts it gates).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Group {
    /// New Design... (button, menu, Ctrl+N, empty-state card).
    NewDesign,
    /// "+ Add Tier", the Tier tab, New Tier, Add/Save Tier.
    TierForm,
    /// The tier table's toolbar and the command bar's Duplicate/Delete/Move.
    TierTable,
    /// The Design Settings panel.
    DesignSettings,
    /// Solve, F5, the auto-solve combo.
    Solve,
    /// The Preform tab and its Apply buttons.
    PreformTab,
    /// Deep Solve, Optimize, Retarget, Snapshot, Tilt Curves.
    Advanced,
    /// Live Render, the catalogue tabs.
    ViewTabs,
    /// Load/Open/Save/Export.
    FileOps,
    /// Undo/Redo.
    History,
}

/// Every [`Group`] -- the closing step's lock set, which blocks nothing.
pub const ALL_GROUPS: &[Group] = &[
    Group::NewDesign,
    Group::TierForm,
    Group::TierTable,
    Group::DesignSettings,
    Group::Solve,
    Group::PreformTab,
    Group::Advanced,
    Group::ViewTabs,
    Group::FileOps,
    Group::History,
];

/// The controls a tier-authoring step leaves usable: the tier form, the tier table's own
/// toolbar (to fix a mistake) and Undo/Redo. Shared with the generated "Build this design"
/// lesson, so the Simple-mode rule below lives in one place.
pub(super) const TIER_STEP: &[Group] = &[Group::TierForm, Group::TierTable, Group::History];

/// [`TIER_STEP`] with the Advanced controls, for a step that asks for "Meets: Exact scale
/// value". The Simple interface leaves that entry out of the Meets list for a new tier, and a
/// step that unlocks the Advanced group shows the full list (the same way the tier tutorials
/// unlock it for the entries Simple hides). The Simple | Advanced switch is unlocked too, so
/// the learner can also switch by hand. The Girdle Facet Preset fills the entry in as well,
/// but a learner who types the girdle by hand needs the full list too.
pub(super) const TIER_STEP_EXACT: &[Group] = &[
    Group::TierForm,
    Group::TierTable,
    Group::Advanced,
    Group::History,
];

/// One step's content -- mirrored field-for-field into `GuideStepData`.
#[derive(Debug)]
pub struct Step {
    /// Heading.
    pub title: &'static str,
    /// One sentence of context.
    pub intro: &'static str,
    /// The numbered actions, one short line each.
    pub actions: &'static [&'static str],
    /// What to look for once done (rendered after "Look for: ").
    pub check: &'static str,
    /// Optional explanation; `""` hides it.
    pub why: &'static str,
    /// The status chip's "Waiting for: ..." text; `""` on a manual step.
    pub waiting: &'static str,
    /// The control outlined while this step is current, or `""` for none -- one of
    /// the targets `guide/tests.rs` lists as recognized.
    pub highlight_target: &'static str,
    /// The completion key reported through `GuideModel.notify` (see
    /// `progress::goal_reached`), or [`super::MANUAL`] for a reading step.
    pub completion: &'static str,
    /// The groups of controls that stay usable; every other group is locked.
    pub allow: &'static [Group],
}

/// The eight index positions every symmetric tier in the example uses: a 96-tooth
/// gear at 8-fold symmetry puts one facet every 12 teeth.
pub const EIGHT_FOLD_INDICES: [f64; 8] = [0.0, 12.0, 24.0, 36.0, 48.0, 60.0, 72.0, 84.0];

/// The walkthrough, in display order.
pub const STEPS: &[Step] = &[
    Step {
        title: "Start a new design",
        intro: "The walkthrough builds its stone from an empty design with a known gear and symmetry.",
        actions: &[
            "Click New Design... on the command bar (or File > New Design...).",
            "Start From: Empty.",
            "Preform Shape: Cylinder \u{2014} Half-Width 1.50, Length / Width 1.00, Depth 1.50.",
            "Index Gear: 96. Symmetry Order: 8. Mirror: on.",
            "Starting Material: (none).",
            "Click Create.",
        ],
        check: "the cylinder preform in the viewport, with a \"no tiers yet\" hint above it.",
        why: "96 teeth at 8-fold symmetry put one facet every 12 teeth: 0, 12, 24, 36, 48, 60, 72, 84. Every symmetric tier below uses that list.",
        waiting: "a new Empty design",
        highlight_target: "new_design_dialog",
        completion: "new_design_created",
        allow: &[Group::NewDesign],
    },
    Step {
        title: "Add the girdle first",
        intro: "The girdle goes in first. Its half-width sets the size of the whole stone.",
        actions: &[
            "Click + Add Tier.",
            "Angle (deg): 90.0 \u{2014} or click Girdle Facet Preset, which sets Angle 90.0 and Meets Exact scale value 1.0 in one click.",
            "Meets: Exact scale value \u{2014} 1.0",
            "Name: G1",
            "Indices: 0, 12, 24, 36, 48, 60, 72, 84",
            "Click Add Tier.",
        ],
        check: "the new G1 row reads G1 in the CODE column.",
        why: "Type exactly 90.0: a 0.0 tier counts as crown and would become a second table. At 90 degrees the scale value 1.0 is the girdle's half-width, so the girdle is 2.0 wide, flat to flat. The pavilion and the crown get a scale value of their own in the next steps. This step shows the Advanced controls because the Simple interface leaves Exact scale value out of the Meets list, so you can pick it there even if you type the angle by hand.",
        waiting: "tier G1 at 90.0 with the eight indices",
        highlight_target: "inspector_tier",
        completion: "tier_named:G1",
        allow: TIER_STEP_EXACT,
    },
    Step {
        title: "Add the pavilion main facets",
        intro: "Eight facets, one per repeat, angled steeply below the girdle.",
        actions: &[
            "Click + Add Tier.",
            "Angle (deg): -40.0",
            "Meets: Exact scale value \u{2014} 0.56",
            "Name: P1",
            "Indices: 0, 12, 24, 36, 48, 60, 72, 84",
            "Click Add Tier.",
        ],
        check: "the P1 row's MARGIN column reads -0.5\u{b0} in red.",
        why: "-40.0 is a typical starting pavilion angle; refine it later with Solve and Optimize. With no material yet, the design uses its default refractive index, 1.54, whose critical angle is 40.5 degrees, so -40.0 sits just below it and reads red for now. Step 6 picks a real material and fixes that. The solver needs one exact scale value in each of the girdle, the pavilion and the crown. For a facet, the scale value is how far its plane sits from the centre of the blank, so 0.56 puts the culet, the point at the bottom, about 0.73 below the centre. This step shows the Advanced controls because the Simple interface leaves Exact scale value out of the Meets list.",
        waiting: "tier P1 at -40.0 with the eight indices",
        highlight_target: "inspector_tier",
        completion: "tier_named:P1",
        allow: TIER_STEP_EXACT,
    },
    Step {
        title: "Add the crown main facets",
        intro: "Eight facets, one per repeat, angled above the girdle.",
        actions: &[
            "Click + Add Tier.",
            "Angle (deg): 34.5",
            "Meets: Exact scale value \u{2014} 0.70",
            "Name: C1",
            "Indices: 0, 12, 24, 36, 48, 60, 72, 84",
            "Click Add Tier.",
        ],
        check: "the C1 row reads C1 in the CODE column.",
        why: "The crown needs an exact scale value of its own. Together with the pavilion's 0.56, 0.70 leaves a thin girdle band, about 0.05 thick, between the crown and the pavilion. This step shows the Advanced controls because the Simple interface leaves Exact scale value out of the Meets list.",
        waiting: "tier C1 at 34.5 with the eight indices",
        highlight_target: "inspector_tier",
        completion: "tier_named:C1",
        allow: TIER_STEP_EXACT,
    },
    Step {
        title: "Add the table",
        intro: "One flat facet on top, not repeated around the gear.",
        actions: &[
            "Click + Add Tier.",
            "Angle (deg): 0.0",
            "Meets: Exact scale value \u{2014} 0.46",
            "Name: T",
            "Indices: leave blank",
            "Click Add Tier.",
        ],
        check: "the tier table lists G1, P1, C1 and T.",
        why: "A flat table faces straight up, so its scale value is its height above the centre of the blank. 0.46 makes the table about 57 percent as wide as the stone. Unspecified vertex does not suit this crown: it has only the eight main facets, so the table would sink to the girdle and cut the whole crown away. This step shows the Advanced controls because the Simple interface leaves Exact scale value out of the Meets list.",
        waiting: "tier T at 0.0 with no indices",
        highlight_target: "inspector_tier",
        completion: "tier_named:T",
        allow: TIER_STEP_EXACT,
    },
    Step {
        title: "Pick a real material",
        intro: "The material sets the refractive index, and with it the critical angle every pavilion facet is judged against.",
        actions: &[
            "In Design Settings, set Material: Diamond.",
            "Click Apply Material.",
        ],
        check: "Eff. RI and Crit. update, and P1's MARGIN column turns green and reads +15.6\u{b0}.",
        why: "Diamond's critical angle (about 24.4 degrees) is much smaller than the default's 40.5 degrees, so -40.0 now sits well above it and reads Safe. A lower-RI material could move P1 to Marginal (amber) or Windows (red).",
        waiting: "Diamond to be applied",
        highlight_target: "design_settings",
        completion: "material:Diamond",
        allow: &[Group::DesignSettings, Group::History],
    },
    Step {
        title: "Solve and check",
        intro: "Solve computes every tier's depth and checks that the result is a closed stone.",
        actions: &[
            "Click Solve on the command bar (or press F5).",
            "Read the status strip at the bottom of the Edit tab.",
        ],
        check: "the status strip reports \"Closed solid\" and the stone's volume.",
        why: "If auto-solve already finished, this step completes by itself. A \"no anchor\" message names a block that has no exact scale value: fix it here with the tier form, or with the tier table's Add Anchor button. A Degenerate or Unbounded message names the tier to check: look at its angle and its Meets setting.",
        waiting: "a solve that closes",
        highlight_target: "solve_button",
        completion: "solved_closed",
        allow: &[
            Group::Solve,
            Group::TierForm,
            Group::TierTable,
            Group::History,
        ],
    },
    Step {
        title: "Check the orbits",
        intro: "Each symmetric tier should repeat all the way around the stone.",
        actions: &[
            "Find the ORBIT column in the tier table.",
            "Check that G1, P1 and C1 each read orbit x8.",
        ],
        check: "no row shows an amber count such as 6/8 orbit.",
        why: "An amber count means an index is missing or mistyped: compare that tier's Indices with 0, 12, 24, 36, 48, 60, 72, 84, or use the ORBIT cell's Complete orbit action.",
        // A reading step: it waits for Next rather than completing on its own, so
        // it does not flash past once the orbits are already complete.
        waiting: "",
        highlight_target: "tier_table",
        completion: super::MANUAL,
        allow: &[
            Group::TierForm,
            Group::TierTable,
            Group::Solve,
            Group::History,
        ],
    },
    Step {
        title: "Yield and rendering (optional)",
        intro: "Give the stone a real size to see its volumetric yield and carat weight.",
        actions: &[
            "Open the inspector's Preform tab.",
            "Girdle Diameter (mm): the stone's real width.",
            "Specific Gravity Override: leave it blank to use Diamond's.",
            "Click Apply Yield Inputs.",
        ],
        check: "Volumetric Yield and Est. Carat Weight show values after the next solve.",
        why: "The carat estimate uses the specific gravity of the material set in Design Settings unless you type an override; neither changes the render. Click Skip step if you do not need real units.",
        waiting: "the yield inputs to be applied",
        highlight_target: "preform_tab",
        completion: "yield_applied",
        allow: &[
            Group::PreformTab,
            Group::Solve,
            Group::ViewTabs,
            Group::History,
        ],
    },
    Step {
        title: "You have a working design",
        intro: "A girdle, pavilion, crown and table that solve to a closed stone: the full worked example.",
        actions: &[
            "Switch to Live Render to see the stone rendered in Diamond.",
            "Save it with Save (Ctrl+S), or keep editing.",
            "Continue with Chapter 8 of the manual: Deep Solve, Optimize, Adopt and Apply.",
        ],
        check: "",
        why: "Every control is unlocked again. Reopen this guide any time from Help > Guide: New Design Walkthrough.",
        waiting: "",
        highlight_target: "",
        completion: super::MANUAL,
        allow: ALL_GROUPS,
    },
];
