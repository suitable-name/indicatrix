//! The saved plans list: refreshing it, renaming and deleting entries.

use super::{
    naming::clean_plan_name,
    open::banner_text,
    show_error, spawn_task,
    store::{self, ListedPlan},
};
use crate::{
    RoughPlanModel, SavedRoughPlanRow,
    gui::rough_plan::{host::Host, run::ResultsSource, session::Session},
};
use slint::{ComponentHandle, ModelRc, VecModel};
use tracing::warn;

/// The list row of a stored plan; `None` for an id a Slint `int` cannot hold.
fn row_of(plan: ListedPlan) -> Option<SavedRoughPlanRow> {
    let id = i32::try_from(plan.id)
        .inspect_err(|_| {
            warn!(
                "Rough planner: saved plan {} has an id too large to list",
                plan.id
            );
        })
        .ok()?;
    Some(SavedRoughPlanRow {
        id,
        name: plan.name.into(),
        summary: plan.summary.into(),
        date: plan.date.into(),
    })
}

/// Reloads the list from the database on a worker thread. Overlapping refreshes are
/// harmless: only the newest answer is shown.
pub(super) fn refresh_list(host: &Host) {
    let seq = {
        let mut session = host.session.borrow_mut();
        session.saved.list_seq += 1;
        session.saved.list_seq
    };
    spawn_task(host, store::list_saved, move |host, result| {
        if host.session.borrow().saved.list_seq != seq {
            return;
        }
        match result {
            Ok(plans) => {
                let rows: Vec<SavedRoughPlanRow> = plans.into_iter().filter_map(row_of).collect();
                host.window
                    .global::<RoughPlanModel>()
                    .set_saved_plans(ModelRc::new(VecModel::from(rows)));
            }
            Err(message) => show_error(host, &message),
        }
    });
}

/// Gives the plan on screen its new name when it is the one that was renamed: in the
/// results' source, and in the banner that waits for the rows of a plan just opened.
/// Returns the plan's creation time when the banner already on screen needs the new name.
fn rename_in_session(session: &mut Session, id: i64, name: &str) -> Option<i64> {
    let ResultsSource::Loaded {
        plan_id,
        name: shown,
        created_at,
    } = &mut session.run.source
    else {
        return None;
    };
    if *plan_id != id {
        return None;
    }
    *shown = name.to_string();
    let created_at = *created_at;
    if let Some(pending) = session
        .saved
        .pending_banner
        .as_mut()
        .filter(|pending| pending.plan_id == id)
    {
        pending.text = banner_text(name, created_at);
        return None;
    }
    Some(created_at)
}

/// A plan on screen keeps its name in the results' source and in the banner above the
/// results; both follow a rename of that plan.
fn follow_rename(host: &Host, id: i64, name: &str) {
    let created_at = rename_in_session(&mut host.session.borrow_mut(), id, name);
    let Some(created_at) = created_at else {
        return;
    };
    let model = host.window.global::<RoughPlanModel>();
    if !model.get_loaded_banner().is_empty() {
        model.set_loaded_banner(banner_text(name, created_at).into());
    }
}

/// `RoughPlanModel.rename_saved`.
pub(super) fn rename_saved(host: &Host, id: i64, new_name: &str) {
    let name = clean_plan_name(new_name);
    if name.is_empty() {
        show_error(host, "A saved plan needs a name.");
        return;
    }
    let stored_name = name.clone();
    spawn_task(
        host,
        move |db| store::rename_saved(db, id, &stored_name),
        move |host, result| {
            match result {
                Ok(()) => follow_rename(host, id, &name),
                Err(message) => show_error(host, &message),
            }
            // A plan that no longer exists must leave the list as well.
            refresh_list(host);
        },
    );
}

/// `RoughPlanModel.delete_saved` (the window asked for confirmation already).
pub(super) fn delete_saved(host: &Host, id: i64) {
    spawn_task(
        host,
        move |db| store::delete_saved(db, id),
        |host, result| {
            if let Err(message) = result {
                show_error(host, &message);
            }
            refresh_list(host);
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gui::rough_plan::saved::PendingBanner;

    fn loaded(plan_id: i64, name: &str) -> ResultsSource {
        ResultsSource::Loaded {
            plan_id,
            name: name.to_string(),
            created_at: 1_790_726_400,
        }
    }

    #[test]
    fn a_rename_reaches_the_plan_on_screen_and_only_that_plan() {
        let mut session = Session::default();
        session.run.source = loaded(4, "Old");
        assert_eq!(rename_in_session(&mut session, 5, "Other"), None);
        assert_eq!(
            session.run.source,
            loaded(4, "Old"),
            "another plan was renamed"
        );
        assert_eq!(
            rename_in_session(&mut session, 4, "New"),
            Some(1_790_726_400),
            "the banner on screen needs the plan's date to be rewritten"
        );
        assert_eq!(session.run.source, loaded(4, "New"));

        session.run.source = ResultsSource::Planned;
        assert_eq!(rename_in_session(&mut session, 4, "Unrelated"), None);
        assert_eq!(session.run.source, ResultsSource::Planned);
    }

    #[test]
    fn a_banner_still_waiting_for_its_rows_gets_the_new_name_itself() {
        let mut session = Session::default();
        session.run.source = loaded(4, "Old");
        session.saved.pending_banner = Some(PendingBanner {
            plan_id: 4,
            text: banner_text("Old", 1_790_726_400),
        });
        assert_eq!(rename_in_session(&mut session, 4, "Newer"), None);
        assert_eq!(
            session
                .saved
                .pending_banner
                .as_ref()
                .map(|p| p.text.as_str()),
            Some(banner_text("Newer", 1_790_726_400).as_str())
        );
        assert_eq!(session.run.source, loaded(4, "Newer"));
    }
}
