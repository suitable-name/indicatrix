//! The number keypad: the `PropsModel` callbacks that drive the keypad sheet of the Stone
//! and Checks tabs and the inline keypad of the New Design form. The typed text lives in a
//! core [`NumberEntry`]; this module only copies its view into `PropsModel`.

use std::{cell::RefCell, rc::Rc};

use indicatrix_mobile_core::design_props::{NumberEntry, NumberView};
use slint::ComponentHandle;

use crate::{AppWindow, PropsModel};

/// Writes `view` into the keypad properties.
fn show(props: &PropsModel, view: NumberView) {
    props.set_number_readout(view.readout.into());
    props.set_number_typing(view.typing);
    props.set_number_text(view.text.into());
}

/// Connects `number-begin` and `number-key-pressed`.
pub(super) fn wire(window: &AppWindow) {
    let entry = Rc::new(RefCell::new(NumberEntry::new()));
    let props = window.global::<PropsModel>();

    props.on_number_begin({
        let (entry, weak) = (Rc::clone(&entry), window.as_weak());
        move |current| {
            if let Some(window) = weak.upgrade() {
                let view = entry.borrow_mut().begin(&current);
                show(&window.global::<PropsModel>(), view);
            }
        }
    });
    props.on_number_key_pressed({
        let (entry, weak) = (entry, window.as_weak());
        move |id| {
            if let Some(window) = weak.upgrade() {
                let view = entry.borrow_mut().press(&id);
                show(&window.global::<PropsModel>(), view);
            }
        }
    });
}
