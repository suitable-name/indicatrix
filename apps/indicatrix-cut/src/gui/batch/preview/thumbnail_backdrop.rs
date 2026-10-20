//! The Thumbnail Backdrop choice (`SettingsModel.thumbnail_backdrop_selected`): the tray
//! catalogue previews render on, shown in Render & Output and in the preview offer.

use super::offer_quality::OfferRecount;
use crate::{
    MainWindow, SettingsModel,
    settings::{SettingsPersister, model::Backdrop},
};
use slint::ComponentHandle;
use std::{rc::Rc, sync::Arc};

/// Shows the stored choice, hands it to the preview renderer, and wires the pills: a pick is
/// saved, shown, handed to the renderer (which changes the preview fingerprint, so stored
/// previews read as outdated once) and makes an open offer count again.
pub(super) fn setup_thumbnail_backdrop(
    ui: &MainWindow,
    settings_store: &Arc<SettingsPersister>,
    recount: &Rc<OfferRecount>,
) {
    let stored = settings_store.snapshot().settings.thumbnail_backdrop;
    ui.global::<SettingsModel>()
        .set_thumbnail_backdrop_index(stored.index());
    crate::bridge::preview_render::set_thumbnail_backdrop_index(stored.index());

    let store = Arc::clone(settings_store);
    let recount = Rc::clone(recount);
    let weak = ui.as_weak();
    ui.global::<SettingsModel>()
        .on_thumbnail_backdrop_selected(move |index| {
            let backdrop = Backdrop::from_index(index);
            store.update(|file| file.settings.thumbnail_backdrop = backdrop);
            crate::bridge::preview_render::set_thumbnail_backdrop_index(backdrop.index());
            if let Some(ui) = weak.upgrade() {
                ui.global::<SettingsModel>()
                    .set_thumbnail_backdrop_index(backdrop.index());
            }
            recount.quality_changed();
        });
}
