//! JNI Back commits are queued without waiting across Android and Slint threads.
use crate::{AppWindow, Navigation};
use jni::{
    EnvUnowned,
    objects::JObject,
    sys::{jfloat, jint},
};
use slint::ComponentHandle;
use std::cell::RefCell;

thread_local! { static WINDOW: RefCell<Option<slint::Weak<AppWindow>>> = const { RefCell::new(None) }; }

pub(crate) fn install(window: &AppWindow) {
    WINDOW.with(|value| *value.borrow_mut() = Some(window.as_weak()));
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_indicatrixsuite_mobile_MainActivity_nativeBackCommitted(
    _env: EnvUnowned<'_>,
    _this: JObject<'_>,
    target: jint,
    version: jint,
) {
    let _queued = slint::invoke_from_event_loop(move || {
        WINDOW.with(|value| {
            if let Some(window) = value.borrow().as_ref().and_then(slint::Weak::upgrade) {
                crate::navigation::back(&window, target, version);
            }
        });
    });
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_indicatrixsuite_mobile_MainActivity_nativeBackProgress(
    _env: EnvUnowned<'_>,
    _this: JObject<'_>,
    target: jint,
    version: jint,
    progress: jfloat,
) {
    let _queued = slint::invoke_from_event_loop(move || {
        WINDOW.with(|value| {
            if let Some(window) = value.borrow().as_ref().and_then(slint::Weak::upgrade) {
                let navigation = window.global::<Navigation>();
                if navigation.get_target() == target && navigation.get_version() == version {
                    navigation.set_gesture_progress(progress.clamp(0.0, 1.0));
                }
            }
        });
    });
}
