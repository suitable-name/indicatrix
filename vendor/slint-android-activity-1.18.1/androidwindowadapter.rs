// Copyright © SixtyFPS GmbH <info@slint.dev>
// SPDX-License-Identifier: GPL-3.0-only OR LicenseRef-Slint-Royalty-free-2.0 OR LicenseRef-Slint-Software-3.0

// cSpell:ignore Eisu Endcall Hankaku Headsethook Henkan Muhenkan Numpad Pictsymbols Sysrq teriary Thumbl Thumbr Zenkaku

use super::*;
use crate::javahelper::{JavaHelper, print_jni_error};
use android_activity::input::{
    ButtonState, InputEvent, KeyAction, Keycode, MotionAction, MotionEvent,
};
use android_activity::{InputStatus, MainEvent, PollEvent};
use i_slint_core::SharedString;
use i_slint_core::api::{
    LogicalPosition, LogicalSize, PhysicalPosition, PhysicalSize, PlatformError, Window,
};
use i_slint_core::input::{InternalKeyEvent, KeyEvent, KeyEventType, TouchPhase};
use i_slint_core::lengths::PhysicalEdges;
use i_slint_core::platform::{
    InternalEvent, Key, PointerEventButton, WindowAdapter, WindowEvent, WindowEventDispatchResult,
    WindowProperties,
};
use i_slint_core::timers::{Timer, TimerMode};
use i_slint_core::window::{InputMethodRequest, WindowInner};
use i_slint_renderer_skia::{SkiaRenderer, SkiaSharedContext};
use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;

struct LongPressDetection {
    _timer: Timer,
    position: LogicalPosition,
}

pub struct AndroidWindowAdapter {
    app: AndroidApp,
    pub(crate) window: Window,
    pub(crate) renderer: i_slint_renderer_skia::SkiaRenderer,
    requested_graphics_api: RefCell<Option<i_slint_core::graphics::RequestedGraphicsAPI>>,
    pub(crate) event_queue: EventQueue,
    pub(crate) pending_redraw: Cell<bool>,
    pub(super) java_helper: JavaHelper,
    pub(crate) fullscreen: Cell<bool>,
    /// The offset at which the Slint view is drawn in the native window (account for status bar)
    pub offset: Cell<PhysicalPosition>,

    /// Whether the cursor handle should be shown.
    /// They are shown when taping, but hidden whenever keys are pressed
    pub(crate) show_cursor_handles: Cell<bool>,

    long_press: RefCell<Option<LongPressDetection>>,
    last_pressed_state: Cell<ButtonState>,
    // Consume the complete legacy Back sequence that dismissed a popup.
    popup_back_pressed: Cell<bool>,
}

impl WindowAdapter for AndroidWindowAdapter {
    fn window(&self) -> &Window {
        &self.window
    }
    fn size(&self) -> PhysicalSize {
        self.app.native_window().map_or_else(Default::default, |w| PhysicalSize {
            width: w.width() as u32,
            height: w.height() as u32,
        })
    }
    fn renderer(&self) -> &dyn i_slint_core::platform::Renderer {
        &self.renderer
    }

    fn request_redraw(&self) {
        self.pending_redraw.set(true);
    }

    fn update_window_properties(&self, properties: WindowProperties<'_>) {
        let f = properties.is_fullscreen();
        if self.fullscreen.replace(f) != f {
            self.resize().unwrap();
        }
    }

    fn internal(
        &self,
        _: i_slint_core::InternalToken,
    ) -> Option<&dyn i_slint_core::window::WindowAdapterInternal> {
        Some(self)
    }
}

impl i_slint_core::window::WindowAdapterInternal for AndroidWindowAdapter {
    #[cfg(feature = "native-activity")]
    fn input_method_request(&self, request: InputMethodRequest) {
        match request {
            InputMethodRequest::Enable(props) => {
                self.java_helper
                    .set_imm_data(
                        &props,
                        self.window.scale_factor(),
                        self.show_cursor_handles.get(),
                    )
                    .unwrap_or_else(|e| print_jni_error(&self.app, e));
                self.java_helper
                    .show_or_hide_soft_input(true)
                    .unwrap_or_else(|e| print_jni_error(&self.app, e));

                if let Some(focus_item) =
                    WindowInner::from_pub(&self.window).focus_item.borrow().upgrade()
                {
                    if let Some(text_input) =
                        focus_item.downcast::<i_slint_core::items::TextInput>()
                    {
                        let color = text_input.as_pin_ref().selection_background_color();
                        self.java_helper
                            .set_handle_color(color.with_alpha(1.))
                            .unwrap_or_else(|e| print_jni_error(&self.app, e));
                    }
                }
            }
            InputMethodRequest::Update(props) => {
                self.java_helper
                    .set_imm_data(
                        &props,
                        self.window.scale_factor(),
                        self.show_cursor_handles.get(),
                    )
                    .unwrap_or_else(|e| print_jni_error(&self.app, e));
            }
            InputMethodRequest::Disable => {
                self.java_helper
                    .show_or_hide_soft_input(false)
                    .unwrap_or_else(|e| print_jni_error(&self.app, e));
            }
            _ => (),
        };
    }

    #[cfg(not(feature = "native-activity"))]
    fn input_method_request(&self, request: InputMethodRequest) {
        use android_activity::input::{TextInputState, TextSpan};

        let props = match request {
            InputMethodRequest::Enable(props) => {
                self.app.show_soft_input(true);
                props
            }
            InputMethodRequest::Update(props) => props,
            InputMethodRequest::Disable => {
                self.app.hide_soft_input(true);
                return;
            }
            _ => return,
        };
        let mut text = props.text.to_string();
        if !props.preedit_text.is_empty() {
            text.insert_str(props.preedit_offset, props.preedit_text.as_str());
        }
        self.app.set_text_input_state(TextInputState {
            text,
            selection: TextSpan {
                start: props.anchor_position.unwrap_or(props.cursor_position),
                end: props.cursor_position,
            },
            compose_region: (!props.preedit_text.is_empty()).then_some(TextSpan {
                start: props.preedit_offset,
                end: props.preedit_offset + props.preedit_text.len(),
            }),
        });
    }

    fn safe_area_inset(&self) -> PhysicalEdges {
        if self.fullscreen.get() {
            Default::default()
        } else {
            self.java_helper.get_safe_area().unwrap_or_else(|e| print_jni_error(&self.app, e))
        }
    }
}

impl AndroidWindowAdapter {
    pub fn new(app: AndroidApp) -> Rc<Self> {
        let java_helper = JavaHelper::new(&app).unwrap_or_else(|e| print_jni_error(&app, e));
        Rc::<Self>::new_cyclic(|w| Self {
            app,
            window: Window::new(w.clone()),
            #[cfg(not(any(feature = "unstable-wgpu-29", feature = "unstable-wgpu-30")))]
            renderer: SkiaRenderer::default(&SkiaSharedContext::default()),
            #[cfg(all(feature = "unstable-wgpu-29", not(feature = "unstable-wgpu-30")))]
            renderer: SkiaRenderer::default_wgpu_29(&SkiaSharedContext::default()),
            #[cfg(feature = "unstable-wgpu-30")]
            renderer: SkiaRenderer::default_wgpu_30(&SkiaSharedContext::default()),
            requested_graphics_api: RefCell::new(None),
            event_queue: Default::default(),
            pending_redraw: Default::default(),
            java_helper,
            fullscreen: Cell::new(false),
            offset: Default::default(),
            show_cursor_handles: Cell::new(false),
            long_press: RefCell::default(),
            last_pressed_state: Cell::new(ButtonState(0)),
            popup_back_pressed: Cell::new(false),
        })
    }

    pub fn process_event(&self, event: &PollEvent<'_>) -> Result<ControlFlow<()>, PlatformError> {
        let queue = std::mem::take(&mut *self.event_queue.lock().unwrap());
        for e in queue {
            match e {
                Event::Quit => return Ok(ControlFlow::Break(())),
                Event::Other(o) => o(),
            }
        }
        #[cfg_attr(slint_nightly_test, allow(non_exhaustive_omitted_patterns))]
        match event {
            PollEvent::Main(MainEvent::InputAvailable) => self.process_inputs()?,
            PollEvent::Main(MainEvent::InitWindow { .. }) => {
                if let Some(w) = self.app.native_window() {
                    let size = PhysicalSize { width: w.width() as u32, height: w.height() as u32 };

                    let scale_factor =
                        self.app.config().density().map(|dpi| dpi as f32 / 160.0).unwrap_or(1.0);

                    if (scale_factor - self.window.scale_factor()).abs() > f32::EPSILON {
                        self.window.dispatch_event_with_result(
                            WindowEvent::ScaleFactorChanged { scale_factor },
                        )?;
                    }

                    self.renderer.set_window_handle(
                        Arc::new(w),
                        Arc::new(DummyDisplayHandle),
                        size,
                        self.requested_graphics_api.borrow().clone(),
                        false,
                    )?;
                    self.resize()?;

                    // Fixes a problem for old Android versions: the soft input always prompt out on startup.
                    #[cfg(feature = "native-activity")]
                    self.java_helper
                        .show_or_hide_soft_input(false)
                        .unwrap_or_else(|e| print_jni_error(&self.app, e));
                }
            }
            PollEvent::Main(
                MainEvent::WindowResized { .. } | MainEvent::ContentRectChanged { .. },
            ) => self.resize()?,
            PollEvent::Main(MainEvent::RedrawNeeded { .. }) => {
                self.pending_redraw.set(false);
                self.do_render()?;
            }
            PollEvent::Main(MainEvent::GainedFocus) => {
                self.window.dispatch_event_with_result(WindowEvent::WindowActiveChanged(true))?;
            }
            PollEvent::Main(MainEvent::LostFocus) => {
                self.popup_back_pressed.set(false);
                self.window.dispatch_event_with_result(WindowEvent::WindowActiveChanged(false))?;
            }
            PollEvent::Main(MainEvent::ConfigChanged { .. }) => {
                let scale_factor =
                    self.app.config().density().map(|dpi| dpi as f32 / 160.0).unwrap_or(1.0);

                if (scale_factor - self.window.scale_factor()).abs() > f32::EPSILON {
                    self.window.dispatch_event_with_result(WindowEvent::ScaleFactorChanged {
                        scale_factor,
                    })?;
                    self.window.dispatch_event_with_result(WindowEvent::Resized {
                        size: self.size().to_logical(scale_factor),
                    })?;
                    WindowInner::from_pub(&self.window).set_window_item_safe_area(
                        self.internal(i_slint_core::InternalToken)
                            .map(|internal| internal.safe_area_inset().to_logical(scale_factor))
                            .unwrap_or_default(),
                    );
                }
            }
            PollEvent::Main(MainEvent::Destroy) => {
                return Ok(ControlFlow::Break(()));
            }
            _ => (),
        }
        Ok(ControlFlow::Continue(()))
    }

    fn process_inputs(&self) -> Result<(), PlatformError> {
        let mut iter =
            self.app.input_events_iter().map_err(|e| PlatformError::Other(e.to_string()))?;
        loop {
            let mut result = Ok(());
            let read_input = iter.next(|event| match event {
                InputEvent::KeyEvent(key_event) => {
                    if key_event.key_code() == Keycode::Back {
                        if self.popup_back_pressed.get() {
                            if key_event.action() == KeyAction::Up {
                                self.popup_back_pressed.set(false);
                            }
                            return InputStatus::Handled;
                        }
                        if key_event.action() == KeyAction::Down
                            && !WindowInner::from_pub(&self.window).active_popups().is_empty()
                        {
                            self.popup_back_pressed.set(true);
                            for event in [
                                WindowEvent::KeyPressed { text: Key::Escape.into() },
                                WindowEvent::KeyReleased { text: Key::Escape.into() },
                            ] {
                                if let Err(error) = self.window.dispatch_event_with_result(event) {
                                    result = Err(error);
                                    break;
                                }
                            }
                            return InputStatus::Handled;
                        }
                    }
                    match map_key_event(key_event) {
                    Some(ev) => match self.window.dispatch_event_with_result(ev) {
                        Ok(WindowEventDispatchResult::Accepted) => InputStatus::Handled,
                        Ok(_) => InputStatus::Unhandled,
                        Err(e) => {
                            result = Err(e);
                            InputStatus::Unhandled
                        }
                    },
                    None => InputStatus::Unhandled,
                    }
                },
                InputEvent::MotionEvent(motion_event) => {
                    let offset = self.offset.get();
                    let scale = self.window.scale_factor();
                    let touch_pos = |p: &android_activity::input::Pointer<'_>| {
                        i_slint_core::lengths::logical_point_from_api(pointer_logical_position(
                            p.x(),
                            p.y(),
                            offset,
                            scale,
                        ))
                    };
                    match motion_event.action() {
                        MotionAction::ButtonPress => {
                            result = self
                                .window
                                .dispatch_event_with_result(WindowEvent::PointerPressed {
                                    position: position_for_event(motion_event, offset, scale),
                                    button: button_for_event(
                                        motion_event,
                                        &self.last_pressed_state,
                                    ),
                                })
                                .map(|_| ());
                            InputStatus::Handled
                        }
                        MotionAction::ButtonRelease => {
                            result = self
                                .window
                                .dispatch_event_with_result(WindowEvent::PointerReleased {
                                    position: position_for_event(motion_event, offset, scale),
                                    button: button_for_event(
                                        motion_event,
                                        &self.last_pressed_state,
                                    ),
                                })
                                .map(|_| ());
                            InputStatus::Handled
                        }
                        MotionAction::Down => {
                            let position = position_for_event(motion_event, offset, scale);

                            self.show_cursor_handles.set(true);
                            let _timer = Timer::default();
                            _timer.start(
                                TimerMode::SingleShot,
                                self.java_helper
                                    .long_press_timeout()
                                    .unwrap_or_else(|e| print_jni_error(&self.app, e)),
                                long_press_timeout,
                            );
                            self.long_press.replace(Some(LongPressDetection { position, _timer }));
                            if let Some(p) = motion_event.pointers().next() {
                                self.window.dispatch_event(WindowEvent::internal(
                                    InternalEvent::Touch {
                                        id: p.pointer_id(),
                                        position: touch_pos(&p),
                                        phase: TouchPhase::Started,
                                    },
                                ));
                            }
                            InputStatus::Handled
                        }
                        MotionAction::Up => {
                            self.long_press.take();
                            if let Some(p) = motion_event.pointers().next() {
                                self.window.dispatch_event(WindowEvent::internal(
                                    InternalEvent::Touch {
                                        id: p.pointer_id(),
                                        position: touch_pos(&p),
                                        phase: TouchPhase::Ended,
                                    },
                                ));
                            }
                            InputStatus::Handled
                        }
                        MotionAction::Move => {
                            let position = position_for_event(motion_event, offset, scale);

                            let mut lp = self.long_press.borrow_mut();
                            let sq = |x| x * x;
                            if lp.as_ref().map_or(false, |lp| {
                                sq(lp.position.x - position.x) + sq(lp.position.y - position.y)
                                    > 100.
                            }) {
                                *lp = None;
                            }
                            drop(lp);

                            for p in motion_event.pointers() {
                                self.window.dispatch_event(WindowEvent::internal(
                                    InternalEvent::Touch {
                                        id: p.pointer_id(),
                                        position: touch_pos(&p),
                                        phase: TouchPhase::Moved,
                                    },
                                ));
                            }
                            InputStatus::Handled
                        }
                        MotionAction::PointerDown => {
                            // A second finger means no long-press.
                            self.long_press.take();
                            let idx = motion_event.pointer_index();
                            if let Some(p) = motion_event.pointers().nth(idx) {
                                self.window.dispatch_event(WindowEvent::internal(
                                    InternalEvent::Touch {
                                        id: p.pointer_id(),
                                        position: touch_pos(&p),
                                        phase: TouchPhase::Started,
                                    },
                                ));
                            }
                            InputStatus::Handled
                        }
                        MotionAction::PointerUp => {
                            let idx = motion_event.pointer_index();
                            if let Some(p) = motion_event.pointers().nth(idx) {
                                self.window.dispatch_event(WindowEvent::internal(
                                    InternalEvent::Touch {
                                        id: p.pointer_id(),
                                        position: touch_pos(&p),
                                        phase: TouchPhase::Ended,
                                    },
                                ));
                            }
                            InputStatus::Handled
                        }
                        MotionAction::HoverMove => {
                            let position = position_for_event(motion_event, offset, scale);
                            let window_event = WindowEvent::PointerMoved { position };
                            result =
                                self.window.dispatch_event_with_result(window_event).map(|_| ());
                            InputStatus::Handled
                        }
                        MotionAction::Cancel | MotionAction::Outside => {
                            self.long_press.take();
                            for p in motion_event.pointers() {
                                self.window.dispatch_event(WindowEvent::internal(
                                    InternalEvent::Touch {
                                        id: p.pointer_id(),
                                        position: touch_pos(&p),
                                        phase: TouchPhase::Cancelled,
                                    },
                                ));
                            }
                            InputStatus::Handled
                        }
                        // Mouse-wheel scrolling is not forwarded (this app is touch-first);
                        // reporting the event as unhandled lets Android treat it as before.
                        // Upstream panics here with `todo!()`, which a wheel or trackpad
                        // would reach.
                        MotionAction::Scroll
                        | MotionAction::HoverEnter
                        | MotionAction::HoverExit => InputStatus::Unhandled,
                        _ => InputStatus::Unhandled,
                    }
                }
                InputEvent::TextEvent(state) => {
                    self.show_cursor_handles.set(false);
                    // remove the pre_edit
                    let event = if let Some(r) = state.compose_region {
                        let adjust =
                            |pos| if pos > r.start { pos - r.start + r.end } else { pos } as i32;
                        let mut key_event = KeyEvent::default();
                        key_event.text = i_slint_core::format!(
                            "{}{}",
                            &state.text[..r.start],
                            &state.text[r.end..]
                        );

                        InternalKeyEvent {
                            event_type: KeyEventType::UpdateComposition,
                            preedit_text: state.text[r.start..r.end].into(),
                            preedit_selection: Some(0..(r.end - r.start) as i32),
                            replacement_range: Some(i32::MIN..i32::MAX),
                            cursor_position: Some(adjust(state.selection.end)),
                            anchor_position: Some(adjust(state.selection.start)),
                            key_event,
                            ..Default::default()
                        }
                    } else {
                        let mut key_event = KeyEvent::default();
                        key_event.text = state.text.as_str().into();
                        InternalKeyEvent {
                            event_type: KeyEventType::CommitComposition,
                            replacement_range: Some(i32::MIN..i32::MAX),
                            cursor_position: Some(state.selection.end as _),
                            anchor_position: Some(state.selection.start as _),
                            key_event,
                            ..Default::default()
                        }
                    };
                    self.window.dispatch_event(WindowEvent::internal(event));
                    InputStatus::Handled
                }
                _ => InputStatus::Unhandled,
            });

            result?;

            if !read_input {
                return Ok(());
            }
        }
    }

    fn resize(&self) -> Result<(), PlatformError> {
        let Some(win) = self.app.native_window() else { return Ok(()) };
        let size = PhysicalSize { width: win.width() as u32, height: win.height() as u32 };

        let scale_factor = self.window.scale_factor();
        self.window.dispatch_event_with_result(WindowEvent::Resized {
            size: size.to_logical(scale_factor),
        })?;
        WindowInner::from_pub(&self.window).set_window_item_safe_area(
            self.internal(i_slint_core::InternalToken)
                .map(|internal| internal.safe_area_inset().to_logical(scale_factor))
                .unwrap_or_default(),
        );
        self.offset.set(Default::default());
        Ok(())
    }

    pub fn do_render(&self) -> Result<(), PlatformError> {
        if let Some(win) = self.app.native_window() {
            let o = self.offset.get();
            let _ = self.renderer.render_transformed_with_post_callback(
                0.,
                (o.x as f32, o.y as f32),
                PhysicalSize { width: win.width() as _, height: win.height() as _ },
                None,
            )?;
        }
        Ok(())
    }

    pub fn set_requested_graphics_api(
        &self,
        requested_graphics_api: Option<i_slint_core::graphics::RequestedGraphicsAPI>,
    ) {
        *self.requested_graphics_api.borrow_mut() = requested_graphics_api;
    }

    pub(super) fn update_window_insets(
        &self,
        window_origin: PhysicalPosition,
        window_size: PhysicalSize,
        safe_area: PhysicalEdges,
        keyboard: PhysicalEdges,
    ) {
        let scale_factor = self.window.scale_factor();
        WindowInner::from_pub(&self.window)
            .set_window_item_safe_area(safe_area.to_logical(scale_factor));

        let window_origin = window_origin.to_logical(scale_factor);
        let window_size = window_size.to_logical(scale_factor);
        let keyboard = keyboard.to_logical(scale_factor);

        // Assume that the keyboard is only on one side.
        let rect = if keyboard.bottom > (0 as i_slint_core::Coord) {
            (
                LogicalPosition::new(
                    window_origin.x,
                    window_origin.y + window_size.height - keyboard.bottom as i_slint_core::Coord,
                ),
                LogicalSize::new(window_size.width, keyboard.bottom as _),
            )
        } else if keyboard.top > (0 as i_slint_core::Coord) {
            (
                LogicalPosition::new(window_origin.x, window_origin.y),
                LogicalSize::new(window_size.width, keyboard.top as _),
            )
        } else if keyboard.left > (0 as i_slint_core::Coord) {
            (
                LogicalPosition::new(window_origin.x, window_origin.y),
                LogicalSize::new(keyboard.left as _, window_size.height),
            )
        } else if keyboard.right > (0 as i_slint_core::Coord) {
            (
                LogicalPosition::new(
                    window_origin.x + window_size.width - keyboard.right as i_slint_core::Coord,
                    window_origin.y,
                ),
                LogicalSize::new(keyboard.right as _, window_size.height),
            )
        } else {
            Default::default()
        };

        self.window.set_virtual_keyboard(rect.0, rect.1, i_slint_core::InternalToken);
    }
}

// Mechanically split from the pinned upstream source; same module and visibility.
include!("androidwindowadapter_input.rs");
