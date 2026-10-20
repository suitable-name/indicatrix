//! Back enters the toolkit first so a dropdown closes before its enclosing page.
use crate::{AppWindow, Navigation};
use slint::{
    ComponentHandle,
    platform::{Key, WindowEvent},
};

pub fn back(window: &AppWindow, target: i32, version: i32) {
    let navigation = window.global::<Navigation>();
    if target == 0 || navigation.get_target() != target || navigation.get_version() != version {
        return;
    }
    // Reserve this commit even when a popup consumes Escape without changing the
    // page. A duplicate queued native callback must not close the underlying page.
    navigation.set_version(version.wrapping_add(1));
    navigation.set_gesture_progress(0.0);
    for event in [
        WindowEvent::KeyPressed {
            text: Key::Escape.into(),
        },
        WindowEvent::KeyReleased {
            text: Key::Escape.into(),
        },
    ] {
        if let Err(error) = window.window().dispatch_event_with_result(event) {
            tracing::warn!(%error, "Back dispatch failed");
            break;
        }
    }
    // AppWindow's Escape handler commits only if no focused popup consumed it.
}
