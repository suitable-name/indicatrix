//! The Variants view's two comparisons: two designs side by side in the compare window, or as
//! a text diff on its own page.
//!
//! Split from `actions` (the buttons that save, rename, delete and open variants), which keeps
//! the helpers both halves share. The same rules hold here: every function runs on the UI
//! thread, the library is read on a worker ([`spawn_job`]) and the continuation finds the
//! shared state again through [`live_deps`]. `VariantsModel.busy` is on from the click until
//! the continuation runs.

use super::{
    Deps,
    actions::{EDITOR_BUSY_TEXT, NO_DESIGN_TEXT, VARIANT_GONE_TEXT, is_busy, set_busy},
    diff::{self, DiffView},
    identity_of, live_deps, refresh, rows,
    rows::Choice,
    spawn_job, store, with_db,
};
use crate::{
    MainWindow, VariantDiffRow, VariantsModel,
    gui::{
        editor::compare::{VariantSide, open_variants_compare},
        show_toast,
        tutorial_events::raise,
    },
};
use indicatrix::optics::materials::GemMaterial;
use indicatrix_cut_core::Design;
use indicatrix_editor::guide::solving_events::VARIANTS_COMPARED;
use indicatrix_vault::model::design_variant::VariantSummary;
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};
use std::{rc::Rc, sync::Arc};

// --- comparing -----------------------------------------------------------------------------------

/// One of the two designs being compared.
#[derive(Clone)]
struct Side {
    choice: Choice,
    label: String,
}

impl Side {
    /// The label the compare window puts over this side.
    fn picture_label(&self) -> String {
        match self.choice {
            Choice::Current => rows::CURRENT_LABEL.to_owned(),
            Choice::Variant(_) => format!("Variant \"{}\"", self.label),
        }
    }
}

/// The side for `choice`, or `None` when it names a variant that is not in `list`.
fn side_for(choice: Choice, list: &[VariantSummary]) -> Option<Side> {
    match choice {
        Choice::Current => Some(Side {
            choice,
            label: rows::CURRENT_LABEL.to_owned(),
        }),
        Choice::Variant(id) => {
            list.iter()
                .find(|variant| variant.variant_id == id)
                .map(|variant| Side {
                    choice,
                    label: variant.name.clone(),
                })
        }
    }
}

/// The design `side` stands for.
fn side_design(
    db: &indicatrix_vault::db::sqlite::Database,
    uuid: &str,
    side: &Side,
    current: &Design,
) -> Result<Design, String> {
    match side.choice {
        Choice::Current => Ok(current.clone()),
        Choice::Variant(id) => store::load_design(db, uuid, id)
            .map(|(_, design)| design)
            .map_err(|problem| format!("\"{}\": {problem}", side.label)),
    }
}

/// How to show the two designs.
#[derive(Clone, Copy, PartialEq, Eq)]
enum CompareKind {
    Pictures,
    Text,
}

/// What a comparison worker brings back.
enum Compared {
    Pictures(Box<VariantSide>, Box<VariantSide>),
    Text(DiffView),
}

pub(super) fn compare_pictures(ui: &MainWindow, deps: &Rc<Deps>) {
    compare(ui, deps, CompareKind::Pictures);
}

pub(super) fn compare_text(ui: &MainWindow, deps: &Rc<Deps>) {
    compare(ui, deps, CompareKind::Text);
}

/// The materials and designs a comparison starts from.
struct CompareStart {
    uuid: String,
    current: Design,
    custom: Vec<GemMaterial>,
}

fn compare(ui: &MainWindow, deps: &Rc<Deps>, kind: CompareKind) {
    if is_busy(ui, deps) {
        return;
    }
    let model = ui.global::<VariantsModel>();
    let chosen = rows::resolve_pair(
        &deps.page.borrow().choices,
        model.get_compare_first(),
        model.get_compare_second(),
    );
    let (first, second) = match chosen {
        Ok(pair) => pair,
        Err(sentence) => return show_toast(ui, sentence, "info"),
    };
    let sides = {
        let page = deps.page.borrow();
        (side_for(first, &page.list), side_for(second, &page.list))
    };
    let (Some(first), Some(second)) = sides else {
        show_toast(ui, VARIANT_GONE_TEXT, "info");
        return refresh(ui, deps);
    };
    let Some(start) = deps.with_state(|state| {
        identity_of(state).uuid.map(|uuid| CompareStart {
            uuid,
            current: state.design.clone(),
            custom: Vec::new(),
        })
    }) else {
        return show_toast(ui, EDITOR_BUSY_TEXT, "info");
    };
    let Some(mut start) = start else {
        return show_toast(ui, NO_DESIGN_TEXT, "info");
    };
    start.custom = super::custom_materials(deps);
    set_busy(ui, deps, true);
    let token = if kind == CompareKind::Text {
        open_diff_page(ui, deps, &first.label, &second.label)
    } else {
        0
    };
    start_comparison(ui, deps, kind, token, (first, second), start);
}

/// Starts the worker that loads both designs and, for text, works out the comparison.
fn start_comparison(
    ui: &MainWindow,
    deps: &Rc<Deps>,
    kind: CompareKind,
    token: u64,
    sides: (Side, Side),
    start: CompareStart,
) {
    let db = Arc::clone(&deps.db);
    let started = spawn_job(
        ui,
        "variant-compare",
        move || {
            let (first, second) = sides;
            let CompareStart {
                uuid,
                current,
                custom,
            } = start;
            let (before, after) = with_db(&db, |db| {
                Ok::<_, String>((
                    side_design(db, &uuid, &first, &current)?,
                    side_design(db, &uuid, &second, &current)?,
                ))
            })?;
            Ok(match kind {
                CompareKind::Pictures => Compared::Pictures(
                    Box::new(VariantSide {
                        design: before,
                        label: first.picture_label(),
                    }),
                    Box::new(VariantSide {
                        design: after,
                        label: second.picture_label(),
                    }),
                ),
                CompareKind::Text => Compared::Text(diff::compare_designs(
                    (&before, &first.label),
                    (&after, &second.label),
                    &custom,
                )),
            })
        },
        move |ui, answer| {
            if let Some(deps) = live_deps() {
                finish_compare(ui, &deps, token, answer);
            }
        },
    );
    if let Err(reason) = started {
        set_busy(ui, deps, false);
        close_diff(ui, deps);
        show_toast(ui, &reason, "error");
    }
}

/// [`compare`]'s continuation.
fn finish_compare(ui: &MainWindow, deps: &Rc<Deps>, token: u64, answer: Result<Compared, String>) {
    set_busy(ui, deps, false);
    match answer {
        Ok(Compared::Pictures(before, after)) => {
            open_variants_compare(ui, *before, *after);
            raise(ui, VARIANTS_COMPARED);
        }
        Ok(Compared::Text(view)) => {
            show_diff(ui, deps, token, &view);
            raise(ui, VARIANTS_COMPARED);
        }
        Err(reason) => {
            if deps.page.borrow().compare_token == token {
                close_diff(ui, deps);
            }
            show_toast(ui, &reason, "error");
            refresh(ui, deps);
        }
    }
}

// --- the text comparison page ------------------------------------------------------------------

/// Swaps the variants for the comparison page, empty and marked busy. Returns the token the
/// answer must bring back.
fn open_diff_page(ui: &MainWindow, deps: &Deps, first: &str, second: &str) -> u64 {
    let token = {
        let mut page = deps.page.borrow_mut();
        page.compare_token = page.compare_token.wrapping_add(1);
        page.compare_token
    };
    let model = ui.global::<VariantsModel>();
    model.set_diff_title(diff::title(first, second).into());
    model.set_diff_summary(SharedString::new());
    model.set_diff_rows(ModelRc::default());
    model.set_diff_busy(true);
    model.set_diff_open(true);
    token
}

/// Shows a finished comparison, unless the page was closed or another one was asked for.
fn show_diff(ui: &MainWindow, deps: &Deps, token: u64, view: &DiffView) {
    if deps.page.borrow().compare_token != token {
        return;
    }
    let rows: Vec<VariantDiffRow> = view
        .rows
        .iter()
        .map(|line| VariantDiffRow {
            kind: line.kind,
            old_line: line.old_line.as_str().into(),
            new_line: line.new_line.as_str().into(),
            text: line.text.as_str().into(),
        })
        .collect();
    let model = ui.global::<VariantsModel>();
    model.set_diff_title(view.title.as_str().into());
    model.set_diff_summary(view.summary.as_str().into());
    model.set_diff_rows(ModelRc::new(VecModel::from(rows)));
    model.set_diff_busy(false);
    model.set_diff_open(true);
}

/// Leaves the text comparison and drops any answer still on its way.
pub(super) fn close_diff(ui: &MainWindow, deps: &Rc<Deps>) {
    {
        let mut page = deps.page.borrow_mut();
        page.compare_token = page.compare_token.wrapping_add(1);
    }
    let model = ui.global::<VariantsModel>();
    model.set_diff_open(false);
    model.set_diff_busy(false);
    model.set_diff_rows(ModelRc::default());
}
