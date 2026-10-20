//! The confirm step's "Picture Quality" sliders: what changing one does to an open offer.
//!
//! The offer's list of designs was computed for the preview size and samples in force when
//! it opened (the stored pictures' fingerprint includes both). When the user moves a slider
//! while a startup or Regenerate offer is open, the list is stale, so this module counts again
//! with the new values: [`OfferRecount::quality_changed`] switches the dialog to "counting"
//! (Generate off) at once, and about [`RECOUNT_DELAY`] after the LAST change a background
//! scan (`spawn_missing_preview_scan`, the one the startup offer and the Regenerate path
//! already use) replaces the offer's ids and count. An import offer holds exactly the imported
//! designs and is never re-counted.
//!
//! The values themselves are persisted by the same `SettingsModel` callbacks the Render &
//! Output tab uses (registered in `wiring`), and `start_batch` reads the settings snapshot when
//! Generate is pressed, so the batch always uses the values on screen.

use super::{
    scan::spawn_missing_preview_scan,
    wiring::{slint_id, slint_len},
};
use crate::{
    BatchModel, MainWindow,
    gui::batch::regenerate_all::{
        ScopeKind, missing_among, missing_choice_label, next_scope_generation, scope_is_current,
    },
    settings::SettingsPersister,
};
use indicatrix_vault::db::sqlite::Database;
use slint::{ComponentHandle, Model, ModelRc, Timer, TimerMode, VecModel, Weak};
use std::{
    rc::Rc,
    sync::{Arc, Mutex},
    time::Duration,
};

/// How long after the last slider change the re-count starts.
pub(super) const RECOUNT_DELAY: Duration = Duration::from_millis(400);

/// Which trigger opened the confirm step.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum OfferKind {
    /// The once-per-session scan of the whole library: the offer IS the scan result.
    Startup,
    /// A Regenerate command: the offer holds every chosen design and the scan result is
    /// restricted to them ("missing or outdated only").
    Regenerate,
    /// An import: exactly the imported designs, whatever the settings.
    Import,
}

impl OfferKind {
    /// The kind of an open offer from the two flags `BatchModel` carries.
    pub(super) const fn of(from_import: bool, regenerate: bool) -> Self {
        if from_import {
            Self::Import
        } else if regenerate {
            Self::Regenerate
        } else {
            Self::Startup
        }
    }

    /// Whether a quality change makes this offer's list stale.
    pub(super) const fn recounts(self) -> bool {
        !matches!(self, Self::Import)
    }
}

/// What a finished re-count changes in the offer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Recount {
    /// The offer's new design list (startup offers only; a Regenerate offer keeps all of
    /// its designs and only its "missing" subset changes).
    pub offer_ids: Option<Vec<i64>>,
    /// The number of designs the new settings would render.
    pub count: usize,
    /// The "missing or outdated only" subset (Regenerate offers only).
    pub missing_ids: Vec<i64>,
}

/// The offer's new state from a whole-library scan done with the new settings. `all` is the
/// designs the offer was opened for.
pub(super) fn recount_from_scan(kind: OfferKind, all: &[i64], scanned: Vec<i64>) -> Recount {
    match kind {
        OfferKind::Startup => Recount {
            count: scanned.len(),
            offer_ids: Some(scanned),
            missing_ids: Vec::new(),
        },
        OfferKind::Regenerate => {
            let missing_ids = missing_among(all, &scanned);
            Recount {
                offer_ids: None,
                count: missing_ids.len(),
                missing_ids,
            }
        }
        OfferKind::Import => Recount {
            offer_ids: None,
            count: all.len(),
            missing_ids: Vec::new(),
        },
    }
}

/// The kind of the confirm step that is open right now, if one is.
fn open_kind(ui: &MainWindow) -> Option<OfferKind> {
    let model = ui.global::<BatchModel>();
    (model.get_preview_visible() && model.get_preview_confirming()).then(|| {
        OfferKind::of(
            model.get_preview_offer_from_import(),
            model.get_preview_offer_regenerate(),
        )
    })
}

/// Writes a finished re-count into the dialog and ends its "counting" state.
fn publish(ui: &MainWindow, kind: OfferKind, recount: &Recount) {
    let model = ui.global::<BatchModel>();
    if let Some(ids) = &recount.offer_ids {
        let ids: Vec<i32> = ids.iter().map(|&id| slint_id(id)).collect();
        model.set_preview_offer_ids(ModelRc::new(VecModel::from(ids)));
        model.set_preview_offer_count(slint_len(recount.count));
    }
    if kind == OfferKind::Regenerate {
        model.set_preview_offer_missing_count(slint_len(recount.count));
        model.set_preview_offer_missing_label(missing_choice_label(Some(recount.count)).into());
        let missing: Vec<i32> = recount.missing_ids.iter().map(|&id| slint_id(id)).collect();
        model.set_preview_offer_missing_ids(ModelRc::new(VecModel::from(missing)));
    }
    model.set_preview_offer_counting(false);
}

/// Starts the background count with the settings as they are NOW.
fn start_scan(
    ui_weak: &Weak<MainWindow>,
    db: &Arc<Mutex<Database>>,
    settings_store: &SettingsPersister,
) {
    let Some(ui) = ui_weak.upgrade() else {
        return;
    };
    let Some(kind) = open_kind(&ui) else {
        return;
    };
    if !kind.recounts() {
        return;
    }
    let all: Vec<i64> = ui
        .global::<BatchModel>()
        .get_preview_offer_ids()
        .iter()
        .map(i64::from)
        .collect();
    let generation = next_scope_generation(ScopeKind::Preview);
    let settings = settings_store.snapshot().settings;
    spawn_missing_preview_scan(
        ui_weak.clone(),
        Arc::clone(db),
        settings.preview_size,
        settings.preview_spp,
        super::wiring::thumbnail_engine(&settings),
        move |ui, scanned| {
            // Dropped when a newer change or a newer dialog superseded this count, or the
            // question was answered meanwhile.
            if !scope_is_current(ScopeKind::Preview, generation) || open_kind(ui) != Some(kind) {
                return;
            }
            publish(ui, kind, &recount_from_scan(kind, &all, scanned));
        },
    );
}

/// The debounced re-count of an open confirm step. One per main window, shared by the two
/// slider callbacks.
pub(super) struct OfferRecount {
    ui_weak: Weak<MainWindow>,
    db: Arc<Mutex<Database>>,
    settings_store: Arc<SettingsPersister>,
    timer: Timer,
}

impl OfferRecount {
    pub(super) fn new(
        ui: &MainWindow,
        db: &Arc<Mutex<Database>>,
        settings_store: &Arc<SettingsPersister>,
    ) -> Rc<Self> {
        Rc::new(Self {
            ui_weak: ui.as_weak(),
            db: Arc::clone(db),
            settings_store: Arc::clone(settings_store),
            timer: Timer::default(),
        })
    }

    /// Called after a picture-quality slider's new value was stored. Does nothing unless a
    /// startup or Regenerate offer is open.
    pub(super) fn quality_changed(&self) {
        let Some(ui) = self.ui_weak.upgrade() else {
            return;
        };
        let Some(kind) = open_kind(&ui) else {
            return;
        };
        if !kind.recounts() {
            return;
        }
        // The list on screen is stale from this moment: a count still running was made with
        // older values, and Generate stays off until the new count lands.
        let _ = next_scope_generation(ScopeKind::Preview);
        let model = ui.global::<BatchModel>();
        model.set_preview_offer_counting(true);
        if kind == OfferKind::Regenerate {
            model.set_preview_offer_missing_count(-1);
            model.set_preview_offer_missing_label(missing_choice_label(None).into());
        }
        let ui_weak = self.ui_weak.clone();
        let db = Arc::clone(&self.db);
        let settings_store = Arc::clone(&self.settings_store);
        // Restarting a running timer is the debounce: only the last change gets to fire.
        self.timer
            .start(TimerMode::SingleShot, RECOUNT_DELAY, move || {
                start_scan(&ui_weak, &db, &settings_store);
            });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_kind_follows_the_dialog_flags() {
        assert_eq!(OfferKind::of(false, false), OfferKind::Startup);
        assert_eq!(OfferKind::of(false, true), OfferKind::Regenerate);
        assert_eq!(OfferKind::of(true, false), OfferKind::Import);
        assert!(OfferKind::Startup.recounts());
        assert!(OfferKind::Regenerate.recounts());
        assert!(!OfferKind::Import.recounts(), "an import offer is exact");
    }

    #[test]
    fn a_startup_offer_becomes_the_new_scan() {
        let recount = recount_from_scan(OfferKind::Startup, &[1, 2, 3], vec![7, 9]);
        assert_eq!(recount.offer_ids, Some(vec![7, 9]));
        assert_eq!(recount.count, 2);
        assert_eq!(recount.missing_ids, [] as [i64; 0]);
    }

    #[test]
    fn a_startup_offer_can_become_empty() {
        let recount = recount_from_scan(OfferKind::Startup, &[1, 2], Vec::new());
        assert_eq!(recount.offer_ids, Some(Vec::new()));
        assert_eq!(recount.count, 0, "nothing to render at these settings");
    }

    #[test]
    fn a_regenerate_offer_keeps_its_designs_and_narrows_the_missing_set() {
        let recount = recount_from_scan(OfferKind::Regenerate, &[5, 6, 7], vec![7, 100, 5]);
        assert_eq!(recount.offer_ids, None, "the chosen designs stay");
        assert_eq!(recount.missing_ids, vec![5, 7], "in the offer's order");
        assert_eq!(recount.count, 2);
    }

    #[test]
    fn a_regenerate_offer_ignores_missing_designs_outside_its_set() {
        let recount = recount_from_scan(OfferKind::Regenerate, &[1, 2], vec![50, 60]);
        assert_eq!(recount.count, 0);
        assert_eq!(recount.missing_ids, [] as [i64; 0]);
    }

    #[test]
    fn an_import_offer_is_never_changed() {
        let recount = recount_from_scan(OfferKind::Import, &[4, 5], vec![9]);
        assert_eq!(recount.offer_ids, None);
        assert_eq!(recount.count, 2);
    }
}
