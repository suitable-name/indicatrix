//! Putting a verdict into `VerdictModel`.
//!
//! [`rows_of`] is the pure half (a [`Verdict`] to the plain rows the popover lists); the
//! `push_*` functions write the model. Nothing here decides anything: the words, the levels
//! and the fixes all come from `indicatrix_editor::verdict`.

use crate::{MainWindow, VerdictModel, VerdictReasonData};
use indicatrix_editor::verdict::{FixAction, Verdict};
use slint::{ComponentHandle, ModelRc, VecModel};

/// One reason as the popover shows it: `VerdictReasonData` with plain strings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ReasonRow {
    /// 0 Good, 1 Check, 2 Problem.
    pub(super) level: i32,
    /// The sentence.
    pub(super) text: String,
    /// The tier row "Show" selects, or -1 for a reason about the whole stone.
    pub(super) tier: i32,
    /// The Fix button's label, or "" when no safe tool exists.
    pub(super) fix_label: String,
    /// What the Fix does, for its tooltip.
    pub(super) fix_hint: String,
}

/// The rows of `verdict`, in its order (worst first).
pub(super) fn rows_of(verdict: &Verdict) -> Vec<ReasonRow> {
    verdict
        .reasons
        .iter()
        .map(|reason| ReasonRow {
            level: reason.level.code(),
            text: reason.text.clone(),
            tier: reason
                .tier
                .and_then(|tier| i32::try_from(tier).ok())
                .unwrap_or(-1),
            fix_label: reason
                .fix
                .as_ref()
                .map(FixAction::label)
                .unwrap_or_default()
                .to_string(),
            fix_hint: reason
                .fix
                .as_ref()
                .map(FixAction::hint)
                .unwrap_or_default()
                .to_string(),
        })
        .collect()
}

/// Shows `verdict`: the badge, the sentence and the reasons. Clears the out-of-date mark.
pub(super) fn push_verdict(ui: &MainWindow, verdict: &Verdict) {
    let model = ui.global::<VerdictModel>();
    let rows: Vec<VerdictReasonData> = rows_of(verdict)
        .into_iter()
        .map(|row| VerdictReasonData {
            level: row.level,
            text: row.text.into(),
            tier: row.tier,
            fix_label: row.fix_label.into(),
            fix_hint: row.fix_hint.into(),
        })
        .collect();
    model.set_level(verdict.level.code());
    model.set_word(verdict.level.word().into());
    model.set_headline(verdict.headline.as_str().into());
    model.set_reasons(ModelRc::new(VecModel::from(rows)));
    model.set_stale(false);
}

/// Hides the badge: there is nothing to judge (no tiers).
pub(super) fn push_hidden(ui: &MainWindow) {
    let model = ui.global::<VerdictModel>();
    model.set_level(-1);
    model.set_word("".into());
    model.set_headline("".into());
    model.set_reasons(ModelRc::new(
        VecModel::from(Vec::<VerdictReasonData>::new()),
    ));
    model.set_stale(false);
    model.set_busy(false);
}

/// Marks the verdict on screen as out of date (or current again).
pub(super) fn set_stale(ui: &MainWindow, stale: bool) {
    ui.global::<VerdictModel>().set_stale(stale);
}

/// Marks a fix as running.
pub(super) fn set_busy(ui: &MainWindow, busy: bool) {
    ui.global::<VerdictModel>().set_busy(busy);
}
