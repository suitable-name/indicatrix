// Copyright © SixtyFPS GmbH <info@slint.dev>
// SPDX-License-Identifier: GPL-3.0-only OR LicenseRef-Slint-Royalty-free-2.0 OR LicenseRef-Slint-Software-3.0

fn callback_update_text<'local>(
    _env: &mut Env<'local>,
    _class: JClass<'local>,
    text: JString<'local>,
    cursor_position: jint,
    anchor_position: jint,
    preedit_start: jint,
    preedit_end: jint,
) -> Result<(), jni::errors::Error> {
    let java_str = text.to_string();
    let decoded: std::borrow::Cow<str> = (&java_str).into();
    let text = SharedString::from(decoded.as_ref());

    let cursor_position = utf16_offset_to_byte_offset_clamped(&text, cursor_position as usize);
    let anchor_position = utf16_offset_to_byte_offset_clamped(&text, anchor_position as usize);
    let preedit_start = utf16_offset_to_byte_offset_clamped(&text, preedit_start as usize);
    let preedit_end = utf16_offset_to_byte_offset_clamped(&text, preedit_end as usize);

    i_slint_core::api::invoke_from_event_loop(move || {
        if let Some(adaptor) = CURRENT_WINDOW.with_borrow(|x| x.upgrade()) {
            adaptor.show_cursor_handles.set(false);
            let event = if preedit_start != preedit_end {
                let adjust = |pos| {
                    if pos <= preedit_start {
                        pos
                    } else if pos >= preedit_end {
                        preedit_start + (pos - preedit_end)
                    } else {
                        preedit_start
                    }
                } as i32;
                let mut key_event = KeyEvent::default();
                key_event.text =
                    i_slint_core::format!("{}{}", &text[..preedit_start], &text[preedit_end..]);

                InternalKeyEvent {
                    event_type: KeyEventType::UpdateComposition,
                    preedit_text: text[preedit_start..preedit_end].into(),
                    preedit_selection: Some(0..(preedit_end - preedit_start) as i32),
                    replacement_range: Some(i32::MIN..i32::MAX),
                    cursor_position: Some(adjust(cursor_position)),
                    anchor_position: Some(adjust(anchor_position)),
                    key_event,
                    ..Default::default()
                }
            } else {
                let mut key_event = KeyEvent::default();
                key_event.text = text;

                InternalKeyEvent {
                    event_type: KeyEventType::CommitComposition,
                    replacement_range: Some(i32::MIN..i32::MAX),
                    cursor_position: Some(cursor_position as _),
                    anchor_position: Some(anchor_position as _),
                    key_event,
                    ..Default::default()
                }
            };
            adaptor.window.dispatch_event(i_slint_core::platform::WindowEvent::internal(event));
        }
    })
    .unwrap();
    Ok(())
}

fn callback_set_night_mode<'local>(
    _env: &mut Env<'local>,
    _class: JClass<'local>,
    night_mode: jint,
) -> Result<(), jni::errors::Error> {
    i_slint_core::api::invoke_from_event_loop(move || {
        if let Some(w) = CURRENT_WINDOW.with_borrow(|x| x.upgrade()) {
            let scheme = match night_mode {
                0x10 => ColorScheme::Light,  // UI_MODE_NIGHT_NO(0x10)
                0x20 => ColorScheme::Dark,   // UI_MODE_NIGHT_YES(0x20)
                0x0 => ColorScheme::Unknown, // UI_MODE_NIGHT_UNDEFINED
                _ => ColorScheme::Unknown,
            };
            let ctx = i_slint_core::window::WindowInner::from_pub(&w.window).context();
            ctx.set_color_scheme(scheme);
            if let Ok(accent) = w.java_helper.accent_color() {
                ctx.set_accent_color(accent);
            }
        }
    })
    .unwrap();
    Ok(())
}

fn callback_set_font_scale<'local>(
    _env: &mut Env<'local>,
    _class: JClass<'local>,
    font_scale: jfloat,
) -> Result<(), jni::errors::Error> {
    // Skip rather than clobber: an OEM/ROM that hands us 0.0 or NaN during a
    // transient configuration change must not zero out the value `bind_context`
    // already set.
    let Some(size) = font_scale_to_logical_length(font_scale) else { return Ok(()) };
    i_slint_core::api::invoke_from_event_loop(move || {
        if let Some(w) = CURRENT_WINDOW.with_borrow(|x| x.upgrade()) {
            let ctx = i_slint_core::window::WindowInner::from_pub(&w.window).context();
            ctx.set_platform_default_font_size(Some(size));
        }
    })
    .unwrap();
    Ok(())
}

fn callback_move_cursor_handle<'local>(
    _env: &mut Env<'local>,
    _class: JClass<'local>,
    id: jint,
    pos_x: jint,
    pos_y: jint,
) -> Result<(), jni::errors::Error> {
    i_slint_core::api::invoke_from_event_loop(move || {
        if let Some(adaptor) = CURRENT_WINDOW.with_borrow(|x| x.upgrade()) {
            if let Some(focus_item) = i_slint_core::window::WindowInner::from_pub(&adaptor.window)
                .focus_item
                .borrow()
                .upgrade()
            {
                if let Some(text_input) = focus_item.downcast::<i_slint_core::items::TextInput>() {
                    let scale_factor = adaptor.window.scale_factor();
                    let adaptor = adaptor.clone() as Rc<dyn WindowAdapter>;
                    let size = text_input
                        .as_pin_ref()
                        .font_request(&focus_item)
                        .pixel_size
                        .unwrap_or_default()
                        .get();
                    let pos =
                        euclid::point2(
                            pos_x as f32 / scale_factor,
                            pos_y as f32 / scale_factor - size / 2.,
                        ) - focus_item.map_to_window(focus_item.geometry().origin).to_vector();
                    let (text_pos, affinity) = text_input.as_pin_ref().byte_offset_for_position(
                        pos,
                        &adaptor,
                        &focus_item,
                    );

                    let (cur_pos, cur_affinity) = if id == 0 {
                        text_input.anchor_position_byte_offset.set(text_pos as i32);
                        (text_pos as i32, affinity)
                    } else {
                        let current_cursor = text_input.as_pin_ref().cursor_position_byte_offset();
                        let current_anchor = text_input.as_pin_ref().anchor_position_byte_offset();
                        if (id == 1 && current_anchor < current_cursor)
                            || (id == 2 && current_anchor > current_cursor)
                        {
                            if current_cursor == text_pos as i32 {
                                return;
                            }
                            text_input.anchor_position_byte_offset.set(text_pos as i32);
                            (current_cursor, text_input.as_pin_ref().cursor_position_affinity())
                        } else {
                            if current_anchor == text_pos as i32 {
                                return;
                            }
                            (text_pos as i32, affinity)
                        }
                    };

                    text_input.as_pin_ref().set_cursor_position_with_affinity(
                        cur_pos,
                        cur_affinity,
                        true,
                        i_slint_core::items::TextChangeNotify::TriggerCallbacks,
                        &adaptor,
                        &focus_item,
                    );
                }
            }
        }
    })
    .unwrap();
    Ok(())
}

fn callback_popup_menu_action<'local>(
    _env: &mut Env<'local>,
    _class: JClass<'local>,
    id: jint,
) -> Result<(), jni::errors::Error> {
    i_slint_core::api::invoke_from_event_loop(move || {
        if let Some(adaptor) = CURRENT_WINDOW.with_borrow(|x| x.upgrade()) {
            if let Some(focus_item) = i_slint_core::window::WindowInner::from_pub(&adaptor.window)
                .focus_item
                .borrow()
                .upgrade()
            {
                if let Some(text_input) = focus_item.downcast::<i_slint_core::items::TextInput>() {
                    let text_input = text_input.as_pin_ref();
                    let adaptor = adaptor.clone() as Rc<dyn WindowAdapter>;
                    match id {
                        0 => text_input.cut(&adaptor, &focus_item),
                        1 => text_input.copy(&adaptor, &focus_item),
                        2 => text_input.paste(&adaptor, &focus_item),
                        3 => text_input.select_all(&adaptor, &focus_item),
                        _ => (),
                    }
                }
            }
        }
    })
    .unwrap();
    Ok(())
}

fn callback_on_back_invoked<'local>(
    _env: &mut Env<'local>,
    _class: JClass<'local>,
) -> Result<(), jni::errors::Error> {
    // Forward Back as a Key.Back KeyPressed + KeyReleased pair; fall back to
    // Activity.finish() if unhandled.
    let _ = i_slint_core::api::invoke_from_event_loop(move || {
        if let Some(adaptor) = CURRENT_WINDOW.with_borrow(|x| x.upgrade()) {
            let text: SharedString = Key::Back.into();
            let pressed = adaptor
                .window
                .dispatch_event_with_result(WindowEvent::KeyPressed { text: text.clone() });
            let released =
                adaptor.window.dispatch_event_with_result(WindowEvent::KeyReleased { text });
            let handled = matches!(pressed, Ok(WindowEventDispatchResult::Accepted))
                || matches!(released, Ok(WindowEventDispatchResult::Accepted));
            if !handled {
                if let Err(e) = adaptor.java_helper.finish_activity() {
                    i_slint_core::debug_log!("finish_activity failed: {e:#?}");
                }
            }
        }
    });
    Ok(())
}

fn callback_set_insets<'local>(
    _env: &mut Env<'local>,
    _class: JClass<'local>,
    window_top: jint,
    window_left: jint,
    window_bottom: jint,
    window_right: jint,
    safe_area_top: jint,
    safe_area_left: jint,
    safe_area_bottom: jint,
    safe_area_right: jint,
    keyboard_top: jint,
    keyboard_left: jint,
    keyboard_bottom: jint,
    keyboard_right: jint,
) -> Result<(), jni::errors::Error> {
    i_slint_core::api::invoke_from_event_loop(move || {
        if let Some(w) = CURRENT_WINDOW.with_borrow(|x| x.upgrade()) {
            w.update_window_insets(
                PhysicalPosition::new(window_left as _, window_top as _),
                PhysicalSize::new(
                    (window_right - window_left) as _,
                    (window_bottom - window_top) as _,
                ),
                PhysicalEdges::new(
                    safe_area_top as _,
                    safe_area_bottom as _,
                    safe_area_left as _,
                    safe_area_right as _,
                ),
                PhysicalEdges::new(
                    keyboard_top as _,
                    keyboard_bottom as _,
                    keyboard_left as _,
                    keyboard_right as _,
                ),
            );
        }
    })
    .unwrap();
    Ok(())
}
