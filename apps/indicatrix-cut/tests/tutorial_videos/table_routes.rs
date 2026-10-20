//! Filmed routes of the tier table's own controls that the tiers lessons teach: the Quick Add
//! buttons of the bar above the table and the ANGLE cell's key and wheel nudges.
//!
//! A Tier form edit that lands on the same value is a different gesture, so none of these sets a
//! value through the form, and every route requires the visible result before it returns.
//!
//! It also holds the sideways wheel turns of the table itself: the default dock is narrower than a
//! row, so the buttons of the columns from Adopt on (Adopt, Pin, Remove, ...) are out of view
//! until the table is turned, and a click at their place in the window would land on whatever
//! lies beside it. Every route that clicks one of those buttons goes through
//! `Film::reveal_table_button`.

use crate::{
    director::Film,
    support::app::pump,
    targets::{self, Spot},
};
use indicatrix_cut::EditorModel;
use slint::{
    ComponentHandle, LogicalPosition, Model,
    platform::{Key, WindowEvent},
};
use std::time::Duration;

/// The component of the bar above the tier table, which holds the Quick Add buttons.
const TOOLBAR_COMPONENT: &str = "TierTableToolbar";

/// The component of the tier table, which holds every ANGLE cell.
const TABLE_COMPONENT: &str = "EditorTierTable";

/// What one key press or one wheel notch moves an angle by, in degrees.
const NUDGE_DEGREES: f64 = 0.1;

/// The wheel distance of one notch, in Slint's convention (positive scrolls up).
const NOTCH: f32 = 120.0;

/// How far a nudged angle may sit from the angle expected, in degrees.
const ANGLE_TOLERANCE: f64 = 1e-6;

/// How long each key press or wheel notch is held on screen.
const NUDGE_HOLD: Duration = Duration::from_millis(350);

/// How far one sideways wheel turn moves the tier table, in logical pixels.
const SIDEWAYS_TURN: f32 = 120.0;

/// How many sideways turns are made before a button is given up on. A row of the table is about
/// 900 px wide (its columns have fixed widths) and the dock's scrolling area is never much
/// narrower than 560 px, so three or four turns bring the last column in.
const SIDEWAYS_TURNS: usize = 8;

impl Film<'_> {
    /// The scrolling area of the tier table: the rectangle its rows are clipped to.
    pub(super) fn tier_table_viewport(&self) -> Result<Spot, String> {
        let table = self
            .view()
            .root
            .query_descendants()
            .match_inherits(TABLE_COMPONENT)
            .find_first()
            .ok_or("the tier table is not visible")?;
        let scroll = table
            .query_descendants()
            .match_inherits("ScrollView")
            .find_first()
            .ok_or("the tier table has no ScrollView")?;
        Ok(Spot::of(&scroll))
    }

    /// Turns the wheel sideways over the tier table, one turn at a time, until `find` names a
    /// button that is on view, and returns that button with the number of turns it took. `find`
    /// is given the scrolling area of the table and picks from what is on screen with
    /// [`Spot::in_view_of`]. The table is looked at before the first turn, so a button that is on
    /// view already costs no scrolling. The default dock is narrower than a row of the table, so
    /// the columns from Adopt on (Adopt, Pin, Detach, Add Anchor, Move, Duplicate, Remove) are out
    /// of view until the table is turned.
    pub(crate) fn reveal_table_button(
        &mut self,
        find: impl Fn(&Self, &Spot) -> Option<Spot>,
    ) -> Result<(Spot, usize), String> {
        let viewport = self.tier_table_viewport()?;
        let mut turns = 0;
        loop {
            if let Some(button) = find(self, &viewport) {
                return Ok((button, turns));
            }
            if turns == SIDEWAYS_TURNS {
                return Err(format!(
                    "the button is not on view in the tier table after {SIDEWAYS_TURNS} \
                     sideways turns of the wheel"
                ));
            }
            self.wheel_across(&viewport, -SIDEWAYS_TURN)?;
            pump(40);
            turns += 1;
        }
    }

    /// Turns the tier table back by `turns` sideways turns ([`Self::reveal_table_button`] reports
    /// them), so that a later route finds its first columns where it left them.
    pub(crate) fn unturn_table(&mut self, turns: usize) -> Result<(), String> {
        if turns == 0 {
            return Ok(());
        }
        let viewport = self.tier_table_viewport()?;
        for _ in 0..turns {
            self.wheel_across(&viewport, SIDEWAYS_TURN)?;
        }
        pump(40);
        Ok(())
    }

    /// Films clicking the Quick Add button `label` (Girdle, Table or Culet) of the bar above the
    /// tier table, and requires exactly one new tier of that name.
    pub(super) fn quick_add_tier(&mut self, label: &str) -> Result<(), String> {
        let editor = self.ui.global::<EditorModel>();
        let before = editor.get_tiers().row_count();
        let button = self.scoped_button(TOOLBAR_COMPONENT, label)?;
        self.click_and_invoke(&button)?;
        pump(80);
        let tiers = editor.get_tiers();
        let named = tiers
            .iter()
            .filter(|row| row.name.as_str() == label)
            .count();
        if tiers.row_count() != before + 1 || named != 1 {
            return Err(format!(
                "Quick Add {label}: expected one new tier called {label:?}, the table went from \
                 {before} to {} rows with {named} of that name",
                tiers.row_count()
            ));
        }
        self.hold(Duration::from_millis(150))
    }

    /// Films the lesson's key nudge: opens the ANGLE cell of `tier` with F2, presses Up `presses`
    /// times (0.1 degrees each) and Enter, and requires the angle to have moved that far.
    pub(super) fn nudge_angle_with_keys(&mut self, tier: &str, presses: u32) -> Result<(), String> {
        let input = self.open_angle_editor(tier)?;
        let row = self.ui.global::<EditorModel>().get_selected_tier_index();
        let before = self.angle_of_row(row)?;
        self.glide_to(input.center())?;
        for _ in 0..presses {
            self.key("Up")?;
            pump(40);
            self.rec.hold(NUDGE_HOLD)?;
        }
        self.key("Return")?;
        pump(80);
        self.require_nudged(tier, row, before, f64::from(presses))
    }

    /// Films the lesson's wheel nudge: the cursor goes to the ANGLE cell of `tier`, Ctrl is held
    /// and the wheel turns `notches` times (up adds 0.1 degrees a notch, down takes it away).
    pub(super) fn nudge_angle_with_wheel(
        &mut self,
        tier: &str,
        notches: i32,
    ) -> Result<(), String> {
        let row = self.film_select_tier(tier)?;
        let before = self.angle_of_row(row)?;
        let cell = self.angle_cell_of_row(row)?;
        let (x, y) = cell.center();
        let position = LogicalPosition::new(x, y);
        self.glide_to((x, y))?;
        let window = self.ui.window();
        window.dispatch_event(WindowEvent::PointerMoved { position });
        window.dispatch_event(WindowEvent::KeyPressed {
            text: Key::Control.into(),
        });
        let delta_y = if notches > 0 { NOTCH } else { -NOTCH };
        for _ in 0..notches.unsigned_abs() {
            window.dispatch_event(WindowEvent::PointerScrolled {
                position,
                delta_x: 0.0,
                delta_y,
            });
            pump(40);
            self.rec.hold(NUDGE_HOLD)?;
        }
        window.dispatch_event(WindowEvent::KeyReleased {
            text: Key::Control.into(),
        });
        pump(80);
        self.require_nudged(tier, row, before, f64::from(notches))
    }

    /// Fails unless the angle of table row `row` moved by `steps` nudges from `before`.
    fn require_nudged(&self, tier: &str, row: i32, before: f64, steps: f64) -> Result<(), String> {
        let after = self.angle_of_row(row)?;
        let expected = steps.mul_add(NUDGE_DEGREES, before);
        if (after - expected).abs() > ANGLE_TOLERANCE {
            return Err(format!(
                "{tier}: the angle reads {after} after {steps} nudge(s) from {before}, expected {expected}"
            ));
        }
        Ok(())
    }

    /// The angle of table row `row`, unrounded.
    fn angle_of_row(&self, row: i32) -> Result<f64, String> {
        let at = usize::try_from(row).map_err(|_| format!("table row {row} does not exist"))?;
        self.ui
            .global::<EditorModel>()
            .get_tiers()
            .row_data(at)
            .ok_or_else(|| format!("table row {row} disappeared"))?
            .angle_full
            .parse::<f64>()
            .map_err(|_| format!("the angle of table row {row} is not a number"))
    }

    /// The button that shows the angle of table row `row` and opens its editor, inside the tier
    /// table and level with the row.
    fn angle_cell_of_row(&self, row: i32) -> Result<Spot, String> {
        let view = self.view();
        let label = format!("Tier {},", row + 1);
        let row_spot = targets::run_label_start(&view, &label)
            .ok_or_else(|| format!("table row {label:?} is not visible"))?;
        let table = view
            .root
            .query_descendants()
            .match_inherits(TABLE_COMPONENT)
            .find_first()
            .ok_or("the tier table is not visible")?;
        let buttons = table
            .query_descendants()
            .match_accessible_role(uiprobe::AccessibleRole::Button)
            .find_all();
        let cell = buttons
            .iter()
            .filter(|button| {
                button
                    .accessible_label()
                    .is_some_and(|text| text.starts_with("Angle "))
            })
            .map(Spot::of)
            .find(|spot| {
                let (_, y) = spot.center();
                spot.usable(view.size) && y >= row_spot.y && y <= row_spot.y + row_spot.h
            });
        cell.ok_or_else(|| format!("the ANGLE cell of {label:?} is not visible"))
    }
}
