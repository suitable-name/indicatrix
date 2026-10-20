//! Shared visible UI gestures used by the scripted tutorial routes.
use crate::{
    director::Film,
    pacing,
    support::app::pump,
    targets::{self, Query, Resolved, Spot},
};
use indicatrix_cut::{
    EditorModel, GuideModel, HistoryModel, ManipulateModel, SolidPreviewModel, SweepModel,
    VariantsModel,
};
use slint::{
    ComponentHandle, LogicalPosition, Model,
    platform::{PointerEventButton, WindowEvent},
};
use std::{collections::HashSet, time::Duration};

/// Whether `outer` covers `inner` (a pixel of slack for rounding).
fn covers(outer: &Spot, inner: &Spot) -> bool {
    outer.x - 1.0 <= inner.x
        && outer.y - 1.0 <= inner.y
        && outer.x + outer.w + 1.0 >= inner.x + inner.w
        && outer.y + outer.h + 1.0 >= inner.y + inner.h
}

/// Collapses spots that expose one control twice (the same rectangle, or one inside the other,
/// such as a button and its caption) into the outer one.
fn distinct(spots: Vec<Spot>) -> Vec<Spot> {
    let keep: Vec<bool> = spots
        .iter()
        .enumerate()
        .map(|(at, spot)| {
            !spots.iter().enumerate().any(|(other_at, other)| {
                other_at != at && covers(other, spot) && (!covers(spot, other) || other_at < at)
            })
        })
        .collect();
    spots
        .into_iter()
        .zip(keep)
        .filter_map(|(spot, kept)| kept.then_some(spot))
        .collect()
}

/// The one spot of `spots`; zero or several are errors that name `what`.
fn one_of(mut spots: Vec<Spot>, what: &str) -> Result<Spot, String> {
    match spots.len() {
        0 => Err(format!("{what} is not visible")),
        1 => Ok(spots.remove(0)),
        count => {
            let places: Vec<String> = spots
                .iter()
                .map(|spot| format!("({:.0}, {:.0})", spot.x, spot.y))
                .collect();
            Err(format!(
                "{what} is ambiguous: {count} visible controls carry it, at {}",
                places.join(", ")
            ))
        }
    }
}

impl Film<'_> {
    pub(super) fn key(&self, chord: &str) -> Result<(), String> {
        uiprobe::view::press(self.ui.window(), chord)
            .map_err(|error| format!("key {chord:?}: {error:?}"))
    }

    /// Presses `chord` `count` times, holding a frame now and then so the movement shows.
    pub(super) fn key_times(&mut self, chord: &str, count: usize) -> Result<(), String> {
        for done in 1..=count {
            self.key(chord)?;
            if done % 16 == 0 {
                self.hold(Duration::from_millis(40))?;
            }
        }
        Ok(())
    }

    /// The visible controls labelled exactly `label` inside the component `component`, each
    /// control once. "Visible" is a test against the window, not against a scroll area inside
    /// `component`: a caller that needs the on-screen rows of a table filters by the scroll
    /// viewport itself.
    pub(super) fn scoped_spots(&self, component: &str, label: &str) -> Vec<Spot> {
        let view = self.view();
        let Some(root) = view
            .root
            .query_descendants()
            .match_inherits(component)
            .find_first()
        else {
            return Vec::new();
        };
        distinct(
            root.query_descendants()
                .find_all()
                .into_iter()
                .filter(|e| e.accessible_label().is_some_and(|text| text == label))
                .map(|e| Spot::of(&e))
                .filter(|spot| spot.usable(view.size))
                .collect(),
        )
    }

    /// The one control labelled `label` inside `component` (a dialog or panel). A label that
    /// the window also carries elsewhere (a tab pill, a table button) is never mistaken for it.
    pub(crate) fn scoped_button(&self, component: &str, label: &str) -> Result<Spot, String> {
        one_of(
            self.scoped_spots(component, label),
            &format!("{label:?} in {component}"),
        )
    }

    pub(super) fn select_open_combo(&self, current: i32, wanted: i32) -> Result<(), String> {
        let key = if wanted < current { "Up" } else { "Down" };
        for _ in 0..(wanted - current).abs() {
            self.key(key)?;
        }
        self.key("Return")
    }

    pub(super) fn viewport(&self) -> Result<Spot, String> {
        targets::run(&self.view(), &Query::Id("SolidViewportView::drag_area"))
            .ok_or_else(|| "Solid viewport pointer surface is missing".into())
    }

    pub(super) fn pointer(&self, x: f32, y: f32) {
        self.ui.window().dispatch_event(WindowEvent::PointerMoved {
            position: LogicalPosition::new(x, y),
        });
        pump(30);
    }

    pub(crate) fn gesture(
        &mut self,
        start: (f32, f32),
        end: (f32, f32),
        handle: Option<i32>,
        cancel: bool,
        fine: bool,
    ) -> Result<(), String> {
        self.glide_to(start)?;
        self.pointer(start.0, start.1);
        let window = self.ui.window();
        if fine {
            window.dispatch_event(WindowEvent::KeyPressed {
                text: slint::platform::Key::Shift.into(),
            });
        }
        window.dispatch_event(WindowEvent::PointerPressed {
            position: LogicalPosition::new(start.0, start.1),
            button: PointerEventButton::Left,
        });
        pump(30);
        if let Some(kind) = handle {
            let model = self.ui.global::<ManipulateModel>();
            if !model.get_dragging() || model.get_hovered_handle() != kind {
                window.dispatch_event(WindowEvent::PointerReleased {
                    position: LogicalPosition::new(start.0, start.1),
                    button: PointerEventButton::Left,
                });
                if fine {
                    window.dispatch_event(WindowEvent::KeyReleased {
                        text: slint::platform::Key::Shift.into(),
                    });
                }
                return Err(format!(
                    "pointer press did not grab handle {kind}: {}",
                    model.get_hint_text()
                ));
            }
        }
        for i in 1..=16_u8 {
            let fraction = f32::from(i) / 16.0;
            let point = (
                (end.0 - start.0).mul_add(fraction, start.0),
                (end.1 - start.1).mul_add(fraction, start.1),
            );
            if let Err(error) = self.glide_to(point) {
                window.dispatch_event(WindowEvent::PointerReleased {
                    position: LogicalPosition::new(point.0, point.1),
                    button: PointerEventButton::Left,
                });
                if fine {
                    window.dispatch_event(WindowEvent::KeyReleased {
                        text: slint::platform::Key::Shift.into(),
                    });
                }
                return Err(error);
            }
            self.pointer(point.0, point.1);
        }
        if cancel {
            self.key("Escape")?;
        }
        window.dispatch_event(WindowEvent::PointerReleased {
            position: LogicalPosition::new(end.0, end.1),
            button: PointerEventButton::Left,
        });
        if fine {
            window.dispatch_event(WindowEvent::KeyReleased {
                text: slint::platform::Key::Shift.into(),
            });
        }
        pump(60);
        if self.ui.global::<ManipulateModel>().get_dragging() {
            return Err("pointer release left a handle drag active".into());
        }
        Ok(())
    }

    pub(super) fn drag_handle(
        &mut self,
        kind: i32,
        distance: f32,
        cancel: bool,
    ) -> Result<(), String> {
        self.drag_handle_with_modifiers(kind, distance, cancel, false)
    }

    pub(super) fn drag_handle_fine(&mut self, kind: i32, distance: f32) -> Result<(), String> {
        self.drag_handle_with_modifiers(kind, distance, false, true)
    }

    pub(super) fn drag_handle_with_modifiers(
        &mut self,
        kind: i32,
        distance: f32,
        cancel: bool,
        fine: bool,
    ) -> Result<(), String> {
        let model = self.ui.global::<ManipulateModel>();
        if !pacing::wait_real(pacing::STEP_TIMEOUT, || model.get_handles_visible()) {
            return Err("drag handles are not ready".into());
        }
        if (kind == 0 && !model.get_angle_visible())
            || (kind == 1 && !model.get_depth_visible())
            || (kind == 2 && !model.get_index_visible())
        {
            return Err(format!("handle {kind} is not available"));
        }
        let viewport = self.viewport()?;
        let (x, y) = match kind {
            0 => (model.get_angle_tip_x(), model.get_angle_tip_y()),
            1 => (model.get_depth_tip_x(), model.get_depth_tip_y()),
            2 => (model.get_index_tip_x(), model.get_index_tip_y()),
            _ => return Err("invalid handle kind".into()),
        };
        let dx = x - model.get_anchor_x();
        let dy = y - model.get_anchor_y();
        let length = dx.hypot(dy);
        if length < 1.0 {
            return Err("drag handle has no usable axis".into());
        }
        let diagram = self.ui.global::<SolidPreviewModel>().get_view_mode() == 3;
        let zoom = if diagram { model.get_view_zoom() } else { 1.0 };
        let (pan_x, pan_y) = if diagram {
            let root = self
                .view()
                .root
                .query_descendants()
                .match_inherits("SolidViewportView")
                .find_first()
                .ok_or("Diagram viewport is missing")?;
            let root_pos = root.absolute_position();
            let root_size = root.size();
            let image = root
                .query_descendants()
                .match_inherits("Image")
                .find_all()
                .into_iter()
                .find(|item| {
                    let size = item.size();
                    root_size.width.mul_add(-zoom, size.width).abs() < 1.0
                        && root_size.height.mul_add(-zoom, size.height).abs() < 1.0
                })
                .ok_or("scaled Diagram image is missing for pan readback")?;
            let image_pos = image.absolute_position();
            (image_pos.x - root_pos.x, image_pos.y - root_pos.y)
        } else {
            (0.0, 0.0)
        };
        let start = (
            x.mul_add(zoom, viewport.x) + pan_x,
            y.mul_add(zoom, viewport.y) + pan_y,
        );
        let end = (
            (distance * dx / length).mul_add(zoom, start.0),
            (distance * dy / length).mul_add(zoom, start.1),
        );
        self.gesture(start, end, Some(kind), cancel, fine)
    }

    pub(crate) fn draw_slice(&mut self) -> Result<(), String> {
        let model = self.ui.global::<ManipulateModel>();
        if !model.get_slice_mode() {
            return Err("Slice is not enabled".into());
        }
        let before = self.ui.global::<EditorModel>().get_tiers().row_count();
        let viewport = self.viewport()?;
        self.gesture(
            (
                viewport.w.mul_add(0.28, viewport.x),
                viewport.h.mul_add(0.40, viewport.y),
            ),
            (
                viewport.w.mul_add(0.72, viewport.x),
                viewport.h.mul_add(0.40, viewport.y),
            ),
            None,
            false,
            false,
        )?;
        if !model.get_provisional_active()
            || self.ui.global::<EditorModel>().get_tiers().row_count() != before
        {
            return Err("Slice line did not create an uncommitted provisional facet".into());
        }
        Ok(())
    }

    pub(super) fn keep_slice(&mut self) -> Result<(), String> {
        let model = self.ui.global::<ManipulateModel>();
        if !model.get_provisional_active() {
            return Err("Keep requires a real provisional slice".into());
        }
        let editor = self.ui.global::<EditorModel>();
        let before = editor.get_tiers().row_count();
        let label = format!("Keep {}", model.get_provisional_label());
        self.press_label(label.trim_end())?;
        if !pacing::wait_real(pacing::STEP_TIMEOUT, || {
            editor.get_tiers().row_count() == before + 1
                && !model.get_provisional_active()
                && !model.get_slice_mode()
        }) {
            return Err(format!(
                "Keep did not commit exactly one slice: {}",
                model.get_hint_text()
            ));
        }
        if editor.get_selected_tier_index() < 0 {
            return Err("Keep did not select the committed tier".into());
        }
        Ok(())
    }

    pub(crate) fn cut_slice_in(&mut self) -> Result<(), String> {
        let model = self.ui.global::<ManipulateModel>();
        if !model.get_provisional_active() {
            return Err("Slice has no provisional facet to cut in".into());
        }
        for distance in [-35.0, 70.0, -105.0, 140.0] {
            self.drag_handle(1, distance, false)?;
            if self.ui.global::<GuideModel>().get_step_done() {
                return Ok(());
            }
            self.hold(Duration::from_millis(200))?;
        }
        Err(format!(
            "depth drags did not cut the provisional facet into the stone: {}",
            model.get_hint_text()
        ))
    }

    /// Clicks the one visible control labelled `label`. Two different controls with the label
    /// (a tab pill and a dialog button, say) fail the video instead of clicking the first.
    pub(crate) fn press_label(&mut self, label: &str) -> Result<(), String> {
        let view = self.view();
        let spots = distinct(
            view.by_label(label)
                .iter()
                .map(Spot::of)
                .filter(|spot| spot.usable(view.size))
                .collect(),
        );
        let spot = one_of(spots, &format!("control {label:?}"))?;
        self.click_and_invoke(&spot)
    }

    /// Picks the row of `tier` and opens its ANGLE cell for editing with F2; the one visible
    /// editor is returned.
    pub(super) fn open_angle_editor(&mut self, tier: &str) -> Result<Spot, String> {
        self.film_select_tier(tier)?;
        uiprobe::view::press(self.ui.window(), "F2")
            .map_err(|error| format!("keyboard input failed: {error:?}"))?;
        pump(60);
        let cells = self
            .view()
            .root
            .query_descendants()
            .match_inherits("TierAngleCell")
            .find_all();
        let inputs: Vec<_> = cells
            .iter()
            .flat_map(|cell| {
                cell.query_descendants()
                    .match_accessible_role(uiprobe::AccessibleRole::TextInput)
                    .find_all()
            })
            .map(|e| Spot::of(&e))
            .filter(|s| s.usable(self.view().size))
            .collect();
        if inputs.len() != 1 {
            return Err(format!(
                "{tier}: F2 exposed {} inline angle editors",
                inputs.len()
            ));
        }
        Ok(inputs[0].clone())
    }

    pub(super) fn inline_angle(&mut self, tier: &str, angle: &str) -> Result<(), String> {
        let input = self.open_angle_editor(tier)?;
        self.click_and_type(&input, angle)?;
        uiprobe::view::press(self.ui.window(), "Return")
            .map_err(|error| format!("keyboard input failed: {error:?}"))?;
        pump(80);
        Ok(())
    }

    pub(super) fn pin_solved_depth(&mut self, tier: &str) -> Result<(), String> {
        let index = self.film_select_tier(tier)?;
        let at = usize::try_from(index).map_err(|_| format!("Pin {tier:?}: no row index"))?;
        let editor = self.ui.global::<EditorModel>();
        let ready = pacing::wait_real(pacing::STEP_TIMEOUT, || {
            !editor.get_solve_running()
                && editor
                    .get_tiers()
                    .row_data(at)
                    .is_some_and(|row| !row.strategy_is_uncertain && !row.mast_full.is_empty())
        });
        if !ready {
            return Err(format!(
                "Pin {tier:?}, row {index}: current certain solved mast unavailable"
            ));
        }
        let before = editor
            .get_tiers()
            .row_data(at)
            .ok_or("Pin row disappeared")?;
        let row = targets::run_label_start(&self.view(), &format!("Tier {},", index + 1))
            .ok_or_else(|| format!("Pin {tier:?}: target row is not visible"))?;
        // The Pin column lies past the Adopt column, beyond the right edge of the table of the
        // default dock, so the table is turned sideways until the one Pin button on the row is
        // in view (a button out of view still has a place in the window, but a click there would
        // land on whatever lies beside the table).
        let (pin, turns) = self
            .reveal_table_button(|film, area| {
                let mut pins = film
                    .view()
                    .by_label("Pin")
                    .into_iter()
                    .map(|e| Spot::of(&e))
                    .filter(|spot| spot.in_view_of(area) && spot.level_with(&row));
                let pin = pins.next()?;
                pins.next().is_none().then_some(pin)
            })
            .map_err(|why| format!("Pin {tier:?}, row {index}: {why}"))?;
        self.hold(pacing::RESULT_HOLD)?;
        self.click_and_invoke(&pin)?;
        let after = editor
            .get_tiers()
            .row_data(at)
            .ok_or("Pinned row disappeared")?;
        let mast = before
            .mast_full
            .parse::<f64>()
            .map_err(|_| "Pin mast is not numeric")?;
        let actual = after
            .constraint_text
            .parse::<f64>()
            .map_err(|_| "Pinned constraint is not numeric")?;
        if after.constraint_kind != 2
            || (mast - actual).abs() > 1e-8
            || before.name != after.name
            || before.angle_full != after.angle_full
            || before.indices_full != after.indices_full
        {
            return Err(format!(
                "Pin {tier:?}: did not preserve the row at solved mast {mast}, actual kind={}, value={actual}",
                after.constraint_kind
            ));
        }
        self.unturn_table(turns)
    }

    /// Opens the verdict and clicks the Fix named `fix`. `solve_first` presses Solve first, for
    /// the step whose actions say so ("Click Solve ... so the verdict is current"); a step that
    /// follows a solve of its own must not solve a second time.
    pub(super) fn verdict_fix(&mut self, fix: &str, solve_first: bool) -> Result<(), String> {
        if solve_first {
            self.film_command(indicatrix_editor::guide::CMD_SOLVE)?;
        }
        let editor = self.ui.global::<EditorModel>();
        if !pacing::wait_real(pacing::STEP_TIMEOUT, || !editor.get_solve_running()) {
            return Err("verdict fix: current solve did not finish".into());
        }
        let Resolved::Found(badge) = targets::resolve(&self.view(), "verdict_badge") else {
            return Err("verdict badge is not visible".into());
        };
        self.click_and_invoke(&badge)?;
        self.hold(pacing::MENU_HOLD)?;
        let prefix = format!("{fix} for Reason ");
        let buttons: Vec<_> = self
            .view()
            .elements()
            .into_iter()
            .filter(|e| e.accessible_label().is_some_and(|s| s.starts_with(&prefix)))
            .map(|e| Spot::of(&e))
            .filter(|s| s.usable(self.view().size))
            .collect();
        if buttons.len() != 1 {
            return Err(format!(
                "verdict fix {fix:?}: expected one reason-scoped action, found {}",
                buttons.len()
            ));
        }
        self.click_and_invoke(&buttons[0])?;
        Ok(())
    }

    pub(super) fn compare_variants(&mut self) -> Result<(), String> {
        let variants = self.ui.global::<VariantsModel>();
        let choices = variants.get_choices();
        let first = choices
            .iter()
            .position(|choice| choice.as_str() == "Variant 1")
            .ok_or("Variant 1 is missing from comparison choices")?;
        for (label, index) in [
            ("First Design to Compare", first),
            ("Second Design to Compare", 0),
        ] {
            let current = if label.starts_with("First") {
                variants.get_compare_first()
            } else {
                variants.get_compare_second()
            };
            let index = i32::try_from(index).map_err(|_| "comparison choice index overflow")?;
            self.press_label(label)?;
            self.select_open_combo(current, index)?;
        }
        if variants.get_compare_first() != i32::try_from(first).unwrap_or(-1)
            || variants.get_compare_second() != 0
        {
            return Err("variant comparison choices did not match their readback".into());
        }
        // The lesson explicitly offers the Advanced text comparison as an alternative.
        // Picture comparison needs a separate captured window and is not faked here.
        self.press_label("Compare Text")?;
        if !pacing::wait_real(pacing::STEP_TIMEOUT, || {
            variants.get_diff_open() && !variants.get_diff_busy()
        }) {
            return Err("variant text comparison did not open".into());
        }
        self.hold(pacing::RESULT_HOLD)?;
        self.press_label("Back to Variants")?;
        Ok(())
    }

    pub(super) fn delete_variant(&mut self, name: &str) -> Result<(), String> {
        let variants = self.ui.global::<VariantsModel>();
        let matches: Vec<_> = variants
            .get_rows()
            .iter()
            .filter(|row| row.name.as_str() == name)
            .collect();
        if matches.len() != 1 {
            return Err(format!("delete {name:?}: expected one persisted row"));
        }
        let key = matches[0].key.clone();
        self.press_label(&format!("Delete {name}"))?;
        if variants.get_form_mode() != 4 {
            return Err("variant Delete did not open confirmation".into());
        }
        let confirm = variants.get_form_ok().to_string();
        self.press_label(&confirm)?;
        if !pacing::wait_real(pacing::STEP_TIMEOUT, || {
            !variants.get_busy() && !variants.get_rows().iter().any(|row| row.key == key)
        }) {
            return Err(format!("delete {name:?}: persisted key remains"));
        }
        Ok(())
    }

    pub(super) fn save_variant(&mut self, name: &str) -> Result<(), String> {
        let variants = self.ui.global::<VariantsModel>();
        let before: HashSet<_> = variants
            .get_rows()
            .iter()
            .map(|r| r.key.to_string())
            .collect();
        self.press_label("Save as Variant...")?;
        let field = targets::text_input(&self.view(), "Variant Name")
            .ok_or("Variant Name input is missing")?;
        self.click_and_type(&field, name)?;
        let save = match targets::run(&self.view(), &Query::Id("VariantForm::tip-button-form-ok")) {
            Some(spot) => spot,
            // The same button by its caption, but only the one inside the variant form.
            None => self.scoped_button("VariantForm", "Save")?,
        };
        self.click_and_invoke(&save)?;
        if !pacing::wait_real(pacing::STEP_TIMEOUT, || {
            !variants.get_busy()
                && variants
                    .get_rows()
                    .iter()
                    .any(|row| row.name.as_str() == name && !before.contains(row.key.as_str()))
        }) {
            return Err(format!(
                "variant {name:?} was not persisted: {}",
                variants.get_form_error()
            ));
        }
        Ok(())
    }

    pub(super) fn open_variant(&mut self, name: &str) -> Result<(), String> {
        let variants = self.ui.global::<VariantsModel>();
        let rows: Vec<_> = variants
            .get_rows()
            .iter()
            .filter(|r| r.name.as_str() == name)
            .collect();
        if rows.len() != 1 {
            return Err(format!(
                "variant {name:?}: expected unique persisted identity, found {}",
                rows.len()
            ));
        }
        let key = rows[0].key.clone();
        self.press_label(&format!("Open {name}"))?;
        if !pacing::wait_real(pacing::STEP_TIMEOUT, || {
            !variants.get_busy()
                && variants
                    .get_rows()
                    .iter()
                    .any(|row| row.key == key && row.opened_from)
        }) {
            return Err(format!(
                "variant key {key:?}: Open did not set Working From"
            ));
        }
        Ok(())
    }

    pub(super) fn history_jump(&mut self, target_position: i32) -> Result<(), String> {
        let history = self.ui.global::<HistoryModel>();
        let rows = history.get_rows();
        let row_idx = rows
            .iter()
            .position(|r| r.position == target_position)
            .ok_or_else(|| format!("history row for position {target_position} not found"))?;
        let row = rows
            .row_data(row_idx)
            .ok_or_else(|| format!("history row {row_idx} disappeared"))?;
        // The row's accessible label is "<title>, <detail>[, now]" (history_tab.slint). Match it
        // inside the History tab only: tier rows, palette rows and candidates are list items too.
        let prefix = format!("{}, {}", row.title, row.detail);
        let view = self.view();
        let tab = view
            .root
            .query_descendants()
            .match_inherits("HistoryTab")
            .find_first()
            .ok_or("the History tab is not visible")?;
        let spots = distinct(
            tab.query_descendants()
                .match_accessible_role(uiprobe::AccessibleRole::ListItem)
                .find_all()
                .into_iter()
                .filter(|e| e.accessible_label().is_some_and(|l| l.starts_with(&prefix)))
                .map(|e| Spot::of(&e))
                .filter(|s| s.usable(view.size))
                .collect(),
        );
        let spot = one_of(spots, &format!("history row {prefix:?}"))?;
        self.click_and_invoke(&spot)?;
        if history.get_current_position() != target_position {
            return Err(format!(
                "history jump to {target_position} did not change position, now {}",
                history.get_current_position()
            ));
        }
        Ok(())
    }

    pub(super) fn open_sweep(&mut self) -> Result<(), String> {
        let sweep = self.ui.global::<SweepModel>();
        if sweep.get_dialog_open() {
            return Ok(());
        }
        // The Edit menu is native and its items cannot be pointed at inside the recorded window;
        // the lesson offers the in-window route: Ctrl+K and the palette row.
        self.film_command("tools.angle_sweep")?;
        if !pacing::wait_real(pacing::STEP_TIMEOUT, || sweep.get_dialog_open()) {
            return Err("Angle Sweep dialog did not open".into());
        }
        self.hold(pacing::MENU_HOLD)?;
        Ok(())
    }
}
