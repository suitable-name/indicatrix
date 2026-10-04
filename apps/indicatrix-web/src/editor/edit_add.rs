//! Quick-add: authoring a new tier from the dock's quick-add panel -- the desktop's three
//! starting-anchor presets (Table, Girdle, Culet) and a small tier form.
//!
//! The form is parsed with `indicatrix_editor::loading::parse_tier_form`, the code behind
//! the desktop's Tier tab, so angle limits, the index shorthand (`6 x8`, `0:12:96`) and the
//! name-collision check behave identically; the edit is `tier_save::tier_save_edit` (a new
//! tier goes in right after the selected row, else at the end). A blank name is filled
//! in with the next free block name (`C1`, `P1`, ...) like the desktop's Save Tier does.

use super::edit::{Dirty, finish_edit, set_selection, with_app};
use crate::{
    TierTableModel,
    app::{
        Ctx,
        push::{MessageKind, show_message},
    },
};
use indicatrix::geometry::meet_solver::MeetConstraint;
use indicatrix_editor::{
    EditorSession,
    loading::{TierFormFields, next_free_block_name, non_integral_index_warning, parse_tier_form},
    session::tier_nudge_label,
    tier_save::{other_tier_names_excluding, tier_save_edit},
    view_model::row_format::first_unresolved_meet_name,
};
use slint::ComponentHandle;

/// The desktop's quick-add presets: `(angle, meets kind, meets text, name)`. Each pins the
/// tier to an exact scale value (kind 2), so a first-time cutter has something solvable to
/// adjust.
const PRESETS: [(&str, i32, &str, &str); 3] = [
    ("0", 2, "0.32", "Table"),
    ("90", 2, "1", "Girdle"),
    ("-0", 2, "0.88", "Culet"),
];

/// The quick-add form's fields, as typed.
struct QuickTier<'a> {
    angle: &'a str,
    /// 0 meet existing, 1 meet named, 2 scale reference (the panel's combo).
    kind: i32,
    meets: &'a str,
    name: &'a str,
    indices: &'a str,
}

/// What a successful add reports.
struct Added {
    index: usize,
    label: String,
    warning: Option<String>,
}

/// Parses `form` and adds the tier after `selected` (or at the end).
fn add_tier_to(
    session: &mut EditorSession,
    selected: Option<usize>,
    form: &QuickTier<'_>,
) -> Result<Added, String> {
    let other_tier_names = other_tier_names_excluding(&session.design, -1);
    let mut tier = parse_tier_form(TierFormFields {
        angle: form.angle,
        constraint_kind: form.kind,
        constraint_text: form.meets,
        name: form.name,
        indices: form.indices,
        gear_teeth_abs: session.design.meta.gear_teeth_abs(),
        imported_meet: None,
        original_notes: None,
        other_tier_names: other_tier_names.clone(),
    })?;
    // An unnamed tier can never be a meet target: name it like Save Tier does.
    if tier.name.is_empty() {
        tier.name = next_free_block_name(tier.angle_deg, &other_tier_names);
    }
    // A name that resolves to nothing would silently degrade inside the solver.
    if let MeetConstraint::MeetNamed(names) = &tier.constraint
        && let Some(bad_name) = first_unresolved_meet_name(&session.design, names)
    {
        return Err(format!(
            "No facet named '{bad_name}' -- check the Meets field."
        ));
    }
    let warning = non_integral_index_warning(&tier.indices);
    if indicatrix_cut_core::design::labelling::name_indicates_pavilion(&tier.name) {
        tier.angle_deg = -tier.angle_deg.abs();
    }
    let (index, edit) = tier_save_edit(&session.design, -1, tier, selected);
    session.apply(edit).map_err(|e| e.to_string())?;
    let label = session
        .design
        .tiers
        .get(index)
        .map_or_else(String::new, |tier| tier_nudge_label(tier, index));
    Ok(Added {
        index,
        label,
        warning,
    })
}

/// Adds the tier `form` describes; on failure the message is shown under the form and as
/// a toast (the desktop's Tier tab does both).
fn add_tier(ctx: &Ctx, form: &QuickTier<'_>) {
    let result = with_app(ctx, |app| {
        let selected = app.selected_tier;
        let outcome = add_tier_to(&mut app.design.as_mut()?.session, selected, form);
        if let Ok(added) = &outcome {
            set_selection(app, Some(added.index));
        }
        Some(outcome)
    });
    let set_error = |text: &str| {
        if let Some(ui) = ctx.ui.upgrade() {
            ui.global::<TierTableModel>()
                .set_quick_add_error(text.into());
        }
    };
    match result {
        None => {}
        Some(Ok(added)) => {
            set_error("");
            finish_edit(ctx, Dirty::one(added.index));
            show_message(ctx, MessageKind::Info, &format!("Added {}", added.label));
            // Shown last so it is the message left on screen: a possibly unintended
            // fractional index is worth more attention than the confirmation.
            if let Some(warning) = added.warning {
                show_message(ctx, MessageKind::Info, &warning);
            }
        }
        Some(Err(message)) => {
            set_error(&message);
            show_message(ctx, MessageKind::Error, &message);
        }
    }
}

/// Wires the quick-add panel's callbacks.
pub fn wire(model: &TierTableModel<'_>, ctx: &Ctx) {
    let c = ctx.clone();
    model.on_add_tier(move |angle, kind, meets, name, indices| {
        add_tier(
            &c,
            &QuickTier {
                angle: &angle,
                kind,
                meets: &meets,
                name: &name,
                indices: &indices,
            },
        );
    });
    let c = ctx.clone();
    model.on_add_preset(move |preset| {
        let Some((angle, kind, meets, name)) =
            usize::try_from(preset).ok().and_then(|i| PRESETS.get(i))
        else {
            return;
        };
        add_tier(
            &c,
            &QuickTier {
                angle,
                kind: *kind,
                meets,
                name,
                indices: "",
            },
        );
    });
}
