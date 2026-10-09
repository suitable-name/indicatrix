//! The link between the Rough Planner and the Rough colour wizard (`zoning` feature only).
//!
//! The planner's host and the locate window's state are private to this module, so everything the
//! wizard needs from them is handed over as plain data and closures
//! ([`PlannerLink`](crate::gui::rough_colour::wizard::context::PlannerLink)). No logic lives
//! here; the wizard and its tests are in `gui::rough_colour::wizard`.

use super::{
    host::{Host, with_host},
    locate,
    run::ResultsSource,
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
        ResultsSource::Planned => (None, "Rough".to_owned()),
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

use std::sync::Arc;
