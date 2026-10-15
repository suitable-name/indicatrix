//! Navigation: destination changes, the width watcher and the toast.

use std::{cell::RefCell, rc::Rc};

use indicatrix_mobile_core::{app_state::AppState, layout::Destination};
use slint::{ComponentHandle, Weak};

use crate::{AppWindow, Shell, push};

/// Wires `Shell`'s navigation callbacks.
pub(super) fn wire(window: &AppWindow, state: &Rc<RefCell<AppState>>) {
    let shell = window.global::<Shell>();

    shell.on_destination_selected({
        let state = Rc::clone(state);
        move |index| select_destination(&state, index)
    });

    shell.on_width_changed({
        let state = Rc::clone(state);
        let weak = window.as_weak();
        move |width| report_width(&state, &weak, width)
    });

    shell.on_toast_dismissed(push::toast::cancel);

    // The first `changed width` may have fired before the callback existed (the size
    // is set right after the window is created), so report the current width once.
    let size = window
        .window()
        .size()
        .to_logical(window.window().scale_factor());
    shell.invoke_width_changed(size.width);
}

/// The user picked a destination in the navigation bar or rail.
fn select_destination(state: &Rc<RefCell<AppState>>, index: i32) {
    if let Some(destination) = Destination::from_index(index) {
        state.borrow_mut().set_destination(destination);
    } else {
        tracing::warn!(index, "ignoring an unknown destination index");
    }
}

/// The window's width changed (first layout, rotation, split screen): recompute the
/// size class and push it when it changed.
fn report_width(state: &Rc<RefCell<AppState>>, weak: &Weak<AppWindow>, width_dp: f32) {
    let changed = state.borrow_mut().set_window_width(width_dp);
    if changed && let Some(window) = weak.upgrade() {
        push::shell::push_window_class(&window, &state.borrow());
    }
}
