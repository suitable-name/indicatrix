//! The link between the Rough Planner and the Rough colour wizard.
//!
//! The planner's host and the locate window's state are private to this module, so everything the
//! wizard needs from them is handed over as plain data and closures
//! ([`PlannerLink`](crate::gui::rough_colour::wizard::context::PlannerLink)). The wizard and its
//! tests are in `gui::rough_colour::wizard`; the only planner-side logic here is remembering
//! which saved plan the shown results were saved as ([`plan_saved`]) and telling an open wizard.

use super::{
    host::{Host, with_host},
    locate,
    run::ResultsSource,
    session::Session,
};
use crate::{
    RoughPlanModel, Zoning,
    gui::rough_colour::wizard::{
        self,
        context::{PlannerContext, PlannerLink},
    },
};
use indicatrix_cut_core::rough_plan::{RoughBase, shape::hull};
use slint::ComponentHandle;
use std::rc::Rc;

/// Registers the planner's "Rough colour..." button.
pub(super) fn setup(host: &Rc<Host>) {
    super::zoning_hooks::setup(host);
    host.window.global::<Zoning>().on_open_rough_colour(|| {
        let running = with_host(|h| h.window.global::<RoughPlanModel>().get_running());
        if running == Some(false) {
            with_host(open);
        }
    });
}

/// Closes the wizard with the planner.
pub(super) fn close() {
    SAVED_HERE.with(|cell| *cell.borrow_mut() = None);
    wizard::close();
}

fn open(host: &Rc<Host>) {
    let link = PlannerLink {
        main: host.main.clone(),
        db: Arc::clone(&host.db),
        context: Rc::new(|| with_host(|h| context_of(h)).flatten()),
        alignment: Rc::new(locate::alignment_snapshot),
        open_locate: Rc::new(|| {
            with_host(locate::open_locate);
        }),
    };
    wizard::open(&link);
}

fn context_of(host: &Host) -> Option<PlannerContext> {
    let session = host.session.borrow();
    let RoughBase::Hull { id, .. } = session.model.base else {
        return None;
    };
    let mesh = hull::mesh(id)?;
    let (plan_id, rough_name) = match &session.run.source {
        ResultsSource::Loaded { plan_id, name, .. } => (Some(*plan_id), name.clone()),
        // Results that were planned here and then saved: the plan saved from them.
        ResultsSource::Planned => saved_here(&session).map_or_else(
            || (None, "Rough".to_owned()),
            |saved| (Some(saved.plan_id), saved.name),
        ),
    };
    let material = usize::try_from(host.window.global::<RoughPlanModel>().get_material_index())
        .ok()
        .and_then(|i| session.choices.get(i).map(|c| c.name.clone()))
        .unwrap_or_default();
    Some(PlannerContext {
        mesh_id: id,
        mesh,
        rough_name,
        plan_id,
        host_material: material,
    })
}

use std::{
    cell::RefCell,
    collections::hash_map::DefaultHasher,
    hash::{Hash, Hasher},
    sync::Arc,
};

/// The plan the shown, freshly planned results were saved as.
///
/// Saving does not turn planned results into a "loaded" plan (the source stays
/// [`ResultsSource::Planned`]), yet the wizard's Accept needs the saved plan's id. The record is
/// valid for exactly the results it was saved from: [`results_fingerprint`] changes as soon as
/// other results are on screen.
#[derive(Debug, Clone)]
struct SavedHere {
    plan_id: i64,
    name: String,
    fingerprint: u64,
}

thread_local! {
    /// The last save of planned results, see [`SavedHere`].
    static SAVED_HERE: RefCell<Option<SavedHere>> = const { RefCell::new(None) };
}

/// A hash of the results on screen (the layouts' totals and stones), to recognise them again.
pub(super) fn results_fingerprint(session: &Session) -> u64 {
    let mut hasher = DefaultHasher::new();
    session.run.layouts.len().hash(&mut hasher);
    for layout in &session.run.layouts {
        layout.total_carat.to_bits().hash(&mut hasher);
        layout.total_volume_mm3.to_bits().hash(&mut hasher);
        for stone in &layout.stones {
            stone.entry_id.hash(&mut hasher);
            stone.carat.to_bits().hash(&mut hasher);
            stone.volume_mm3.to_bits().hash(&mut hasher);
        }
    }
    hasher.finish()
}

/// The save of the planned results on screen, if there is one and they are still on screen.
fn saved_here(session: &Session) -> Option<SavedHere> {
    let saved = SAVED_HERE.with(|cell| cell.borrow().clone())?;
    (saved.fingerprint == results_fingerprint(session)).then_some(saved)
}

/// The planner stored the results that had `fingerprint` when the save started, as saved plan
/// `plan_id` called `name`. When they are still the results on screen, an open wizard learns the
/// plan id at once (Accept becomes available); other results, or a plan that was loaded, change
/// nothing.
pub(super) fn plan_saved(host: &Rc<Host>, plan_id: i64, name: &str, fingerprint: u64) {
    let current = {
        let session = host.session.borrow();
        let planned = matches!(session.run.source, ResultsSource::Planned);
        (planned && results_fingerprint(&session) == fingerprint).then_some(fingerprint)
    };
    let Some(fingerprint) = current else {
        return;
    };
    SAVED_HERE.with(|cell| {
        *cell.borrow_mut() = Some(SavedHere {
            plan_id,
            name: name.to_owned(),
            fingerprint,
        });
    });
    wizard::plan_saved();
}
