//! Moves a typed, generation-checked concave draft between the shared parser and UI.

use super::Shared;
use crate::{AppWindow, ConcaveModel, push::toast};
use indicatrix_editor::loading::ConcaveTierFormFields;
use indicatrix_mobile_core::concave::ConcaveDraft;
use slint::ComponentHandle;
use std::{cell::RefCell, rc::Rc};

const TOOLS: [&str; 5] = ["CYL", "CON", "CIR", "DSC", "SPH"];
thread_local! { static DRAFT: RefCell<Option<ConcaveDraft>> = const { RefCell::new(None) }; }

pub fn wire(window: &AppWindow, shared: &Shared) {
    let (weak, state) = (window.as_weak(), Rc::clone(shared));
    window.global::<ConcaveModel>().on_create(move || {
        if let Some(window) = weak.upgrade() {
            open(&window, &state, None);
        }
    });
    let (weak, state) = (window.as_weak(), Rc::clone(shared));
    window.global::<ConcaveModel>().on_edit(move |index| {
        if let (Some(window), Ok(index)) = (weak.upgrade(), usize::try_from(index)) {
            open(&window, &state, Some(index));
        }
    });
    for remove in [false, true] {
        let (weak, state) = (window.as_weak(), Rc::clone(shared));
        let apply = move || {
            let Some(window) = weak.upgrade() else { return };
            let result = DRAFT.with(|draft| {
                let mut draft = draft.borrow_mut();
                let draft = draft
                    .as_mut()
                    .ok_or_else(|| "Reopen the tier form.".to_owned())?;
                if remove {
                    draft.remove(&mut state.borrow_mut())
                } else {
                    draft.fields = read_fields(&window);
                    draft.save(&mut state.borrow_mut())
                }
            });
            match result {
                Ok(()) => {
                    close(&window);
                    super::after_edit(&window, &state);
                }
                Err(error) => window.global::<ConcaveModel>().set_error(error.into()),
            }
        };
        if remove {
            window.global::<ConcaveModel>().on_remove(apply);
        } else {
            window.global::<ConcaveModel>().on_save(apply);
        }
    }
    let weak = window.as_weak();
    window.global::<ConcaveModel>().on_close(move || {
        if let Some(window) = weak.upgrade() {
            close(&window);
        }
    });
}

pub fn open(window: &AppWindow, shared: &Shared, index: Option<usize>) {
    {
        let mut app = shared.borrow_mut();
        app.selected_tier = None;
        app.session.selected_concave = index;
        app.session.multi_selected.clear();
    }
    let draft = match ConcaveDraft::capture(&shared.borrow(), index) {
        Ok(draft) => draft,
        Err(error) => {
            toast::show(window, &error);
            return;
        }
    };
    let fields = &draft.fields;
    let model = window.global::<ConcaveModel>();
    model.set_name(fields.name.as_str().into());
    model.set_angle(fields.angle_deg.as_str().into());
    model.set_indices(fields.indices.as_str().into());
    model.set_instructions(fields.instructions.as_str().into());
    model.set_tool(
        i32::try_from(
            TOOLS
                .iter()
                .position(|tool| *tool == fields.tool)
                .unwrap_or(0),
        )
        .unwrap_or(0),
    );
    model.set_azimuth(fields.tool_azimuth_deg.as_str().into());
    model.set_x(fields.x.as_str().into());
    model.set_y(fields.y.as_str().into());
    model.set_z(fields.z.as_str().into());
    model.set_diameter(fields.diameter_ratio.as_str().into());
    model.set_tool_angle(fields.tool_angle_deg.as_str().into());
    model.set_reciprocating(fields.reciprocating);
    model.set_existing(index.is_some());
    model.set_error("".into());
    DRAFT.with(|value| *value.borrow_mut() = Some(draft));
    model.set_open(true);
}

fn read_fields(window: &AppWindow) -> ConcaveTierFormFields {
    let model = window.global::<ConcaveModel>();
    ConcaveTierFormFields {
        name: model.get_name().to_string(),
        angle_deg: model.get_angle().to_string(),
        indices: model.get_indices().to_string(),
        instructions: model.get_instructions().to_string(),
        tool: usize::try_from(model.get_tool())
            .ok()
            .and_then(|index| TOOLS.get(index))
            .unwrap_or(&"CYL")
            .to_string(),
        tool_azimuth_deg: model.get_azimuth().to_string(),
        x: model.get_x().to_string(),
        y: model.get_y().to_string(),
        z: model.get_z().to_string(),
        diameter_ratio: model.get_diameter().to_string(),
        tool_angle_deg: model.get_tool_angle().to_string(),
        reciprocating: model.get_reciprocating(),
    }
}

fn close(window: &AppWindow) {
    DRAFT.with(|draft| *draft.borrow_mut() = None);
    window.global::<ConcaveModel>().set_open(false);
}

pub fn shutdown() {
    DRAFT.with(|draft| *draft.borrow_mut() = None);
}
