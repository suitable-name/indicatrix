//! The question asked after an import -- generate previews now, and how -- and the
//! remembered answer that skips it.

use super::wiring::open_offer;
use crate::{
    BatchModel, MainWindow,
    settings::{ImportPreviewChoice, SettingsPersister},
};
use slint::ComponentHandle;
use std::sync::Arc;

/// The code `BatchModel::preview_import_choice` carries for `choice`.
#[must_use]
pub const fn choice_code(choice: ImportPreviewChoice) -> i32 {
    match choice {
        ImportPreviewChoice::Ask => 0,
        ImportPreviewChoice::Full => 1,
        ImportPreviewChoice::Solid => 2,
        ImportPreviewChoice::Skip => 3,
    }
}

/// Shows the saved choice to the UI. Called once, at startup.
pub fn show_saved_choice(ui: &MainWindow, settings_store: &Arc<SettingsPersister>) {
    let choice = settings_store.snapshot().settings.import_preview_choice;
    ui.global::<BatchModel>()
        .set_preview_import_choice(choice_code(choice));
}

/// Stores `choice` as the answer to every later import and tells the UI.
pub fn remember_choice(
    ui: &MainWindow,
    settings_store: &Arc<SettingsPersister>,
    choice: ImportPreviewChoice,
) {
    settings_store.update(|s| s.settings.import_preview_choice = choice);
    ui.global::<BatchModel>()
        .set_preview_import_choice(choice_code(choice));
}

/// What to do for the `ids` an import just created, following the remembered choice:
/// ask (opening the question with the quick and remember options), start the full
/// batch, start the solid batch, or do nothing. A no-op for an empty `ids`.
pub fn offer_import_previews(ui: &MainWindow, ids: &[i64]) {
    if ids.is_empty() {
        return;
    }
    let model = ui.global::<BatchModel>();
    match model.get_preview_import_choice() {
        1 | 2 => {
            open_offer(ui, ids, true);
            // The answer is already stored; do not store it again.
            model.set_preview_remember_choice(false);
            if model.get_preview_import_choice() == 1 {
                model.invoke_preview_generate_confirmed();
            } else {
                model.invoke_preview_generate_solid_confirmed();
            }
        }
        3 => {}
        _ => open_offer(ui, ids, true),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_choice_has_its_own_code() {
        let codes = [
            ImportPreviewChoice::Ask,
            ImportPreviewChoice::Full,
            ImportPreviewChoice::Solid,
            ImportPreviewChoice::Skip,
        ]
        .map(choice_code);
        assert_eq!(codes, [0, 1, 2, 3]);
    }
}
