//! The designs the planner leaves out: reading the list from the library database,
//! showing it in the window, and the Exclude and Restore actions.
//!
//! The mark itself lives in the library database (`Database::set_planner_excluded`); the
//! session keeps the list as last read. The window mirrors it in three places: the
//! "Excluded designs" list under the candidate choice, the Exclude pill on every design
//! row of the results, and the two design counts (whose query reads the marks itself).

use super::{
    counts::{is_remote, refresh_counts},
    format::to_i32,
    host::{Host, on_host},
    saved::{announce, show_error},
};
use crate::{RoughPlanModel, RoughPlanStoneGroup};
use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel};
use std::{
    collections::{BTreeMap, BTreeSet},
    rc::Rc,
    sync::PoisonError,
};

/// Shown when the exclusions are changed while a remote library is active: the marks
/// belong to the local library.
const REMOTE_REFUSAL: &str = "Switch to the local library to change which designs are planned.";

/// Shown after every excluded design was restored.
const RESTORED_ALL_MESSAGE: &str = "All designs can be planned again.";

/// Registers the Exclude, Restore and Restore all callbacks on the planner window.
pub(super) fn setup_callbacks(host: &Rc<Host>) {
    let model = host.window.global::<RoughPlanModel>();
    model.on_toggle_exclude(|entry_id| on_host(|host| toggle(host, entry_id)));
    model.on_restore_excluded(|entry_id| on_host(|host| restore(host, entry_id)));
    model.on_restore_all_excluded(|| on_host(restore_all));
}

/// The excluded designs with their titles, as the library database has them now. The
/// database lock is taken and released in here; nothing touches the window meanwhile.
fn read_excluded(host: &Host) -> Result<BTreeMap<i64, String>, String> {
    let db = host.db.lock().unwrap_or_else(PoisonError::into_inner);
    let ids: Vec<i64> = db
        .planner_excluded_ids()
        .map_err(|e| format!("{e:#}"))?
        .into_iter()
        .collect();
    db.entry_titles_for(&ids).map_err(|e| format!("{e:#}"))
}

/// The excluded designs in the order the window lists them: by title without regard to
/// case, then by id. A design whose id the window's `int` cannot hold is not a library
/// entry and is left out.
fn display_order(excluded: &BTreeMap<i64, String>) -> Vec<(i32, &str)> {
    let mut order: Vec<(&str, i64)> = excluded
        .iter()
        .map(|(&id, title)| (title.as_str(), id))
        .collect();
    order.sort_by_cached_key(|&(title, id)| (title.to_lowercase(), id));
    order
        .into_iter()
        .filter_map(|(title, id)| i32::try_from(id).ok().map(|id| (id, title)))
        .collect()
}

/// Reads the excluded designs from the library and shows them: the list under the
/// candidate choice, its count and the Exclude pills of the rows on screen. On a database
/// error the message is shown and the list stays as it was.
pub(super) fn reload(host: &Rc<Host>) {
    let excluded = match read_excluded(host) {
        Ok(excluded) => excluded,
        Err(message) => {
            show_error(
                host,
                &format!("Could not read the excluded designs: {message}"),
            );
            return;
        }
    };
    let (ids, names): (Vec<i32>, Vec<SharedString>) = display_order(&excluded)
        .into_iter()
        .map(|(id, title)| (id, SharedString::from(title)))
        .unzip();
    host.session.borrow_mut().excluded = excluded;
    let model = host.window.global::<RoughPlanModel>();
    model.set_excluded_count(to_i32(ids.len()));
    model.set_excluded_ids(ModelRc::new(VecModel::from(ids)));
    model.set_excluded_names(ModelRc::new(VecModel::from(names)));
    sync_row_flags(host);
}

/// The exclusions changed, here or in the library window: the list and the row flags are
/// read again and, unless a plan runs, so are the design counts.
pub(super) fn changed(host: &Rc<Host>) {
    reload(host);
    if host.window.global::<RoughPlanModel>().get_running() {
        return;
    }
    if let Some(main) = host.main.upgrade() {
        refresh_counts(&main, &host.window, &host.db, &host.source, &host.session);
    }
}

/// Sets the `excluded` flag of every design group in `groups` to whether the group's
/// design is in `excluded`. A group that already shows the right flag is not written.
fn flag_groups(groups: &ModelRc<RoughPlanStoneGroup>, excluded: &BTreeSet<i64>) {
    for index in 0..groups.row_count() {
        let Some(mut group) = groups.row_data(index) else {
            continue;
        };
        let flag = excluded.contains(&i64::from(group.entry_id));
        if group.excluded != flag {
            group.excluded = flag;
            groups.set_row_data(index, group);
        }
    }
}

/// Brings the Exclude pill of every design row of the results on screen in line with the
/// session's list of excluded designs.
pub(super) fn sync_row_flags(host: &Rc<Host>) {
    let excluded: BTreeSet<i64> = host.session.borrow().excluded.keys().copied().collect();
    let results = host.window.global::<RoughPlanModel>().get_results();
    for index in 0..results.row_count() {
        if let Some(row) = results.row_data(index) {
            flag_groups(&row.groups, &excluded);
        }
    }
}

/// Whether the exclusions may change now. A remote library is refused with a message (the
/// marks belong to the local library); a running plan is refused quietly (it works on the
/// designs it started with).
fn may_change(host: &Rc<Host>) -> bool {
    if is_remote(&host.source) {
        show_error(host, REMOTE_REFUSAL);
        return false;
    }
    !host.window.global::<RoughPlanModel>().get_running()
}

/// Writes the mark of every design in `ids` under one database lock, released on return.
fn write_marks(host: &Host, ids: &[i64], excluded: bool) -> Result<(), String> {
    let db = host.db.lock().unwrap_or_else(PoisonError::into_inner);
    ids.iter().try_for_each(|&id| {
        db.set_planner_excluded(id, excluded)
            .map_err(|e| format!("Could not change the planner exclusion: {e:#}"))
    })
}

/// Writes the mark of every design in `ids`, then shows the result. Returns `false`, with
/// the error shown and nothing re-read, when the write failed.
fn apply(host: &Rc<Host>, ids: &[i64], excluded: bool) -> bool {
    if let Err(message) = write_marks(host, ids, excluded) {
        show_error(host, &message);
        return false;
    }
    changed(host);
    true
}

/// The title to call design `id` by: the library's when it is excluded, else the one the
/// results on screen carry for it, else its number.
fn title_of(host: &Host, id: i64) -> String {
    let session = host.session.borrow();
    session
        .excluded
        .get(&id)
        .or_else(|| session.run.titles.get(&id))
        .cloned()
        .unwrap_or_else(|| format!("Design #{id}"))
}

/// What the status line and the toast say after one design was excluded or restored.
fn announcement(title: &str, excluded: bool) -> String {
    if excluded {
        format!("Excluded \"{title}\" from planning. Plan again to see new layouts.")
    } else {
        format!("\"{title}\" can be planned again.")
    }
}

/// Excludes or restores one design and says so.
fn set_excluded(host: &Rc<Host>, id: i64, excluded: bool) {
    let before = title_of(host, id);
    if !apply(host, &[id], excluded) {
        return;
    }
    // An excluded design now carries the title the library gave it; a restored one is
    // gone from the list, so it is named as it was.
    let title = host
        .session
        .borrow()
        .excluded
        .get(&id)
        .cloned()
        .unwrap_or(before);
    announce(host, &announcement(&title, excluded));
}

/// `RoughPlanModel.toggle_exclude`: excludes design `entry_id`, or restores it when it is
/// excluded already.
fn toggle(host: &Rc<Host>, entry_id: i32) {
    if !may_change(host) {
        return;
    }
    let id = i64::from(entry_id);
    let excluded = !host.session.borrow().excluded.contains_key(&id);
    set_excluded(host, id, excluded);
}

/// `RoughPlanModel.restore_excluded`: takes design `entry_id` back into the planner.
fn restore(host: &Rc<Host>, entry_id: i32) {
    if may_change(host) {
        set_excluded(host, i64::from(entry_id), false);
    }
}

/// `RoughPlanModel.restore_all_excluded`: takes every excluded design back into the
/// planner.
fn restore_all(host: &Rc<Host>) {
    if !may_change(host) {
        return;
    }
    let ids: Vec<i64> = host.session.borrow().excluded.keys().copied().collect();
    if !ids.is_empty() && apply(host, &ids, false) {
        announce(host, RESTORED_ALL_MESSAGE);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn titles(entries: &[(i64, &str)]) -> BTreeMap<i64, String> {
        entries
            .iter()
            .map(|&(id, title)| (id, title.to_string()))
            .collect()
    }

    fn group(entry_id: i32, excluded: bool) -> RoughPlanStoneGroup {
        RoughPlanStoneGroup {
            entry_id,
            excluded,
            ..RoughPlanStoneGroup::default()
        }
    }

    fn flags(groups: &ModelRc<RoughPlanStoneGroup>) -> Vec<(i32, bool)> {
        (0..groups.row_count())
            .filter_map(|index| groups.row_data(index))
            .map(|group| (group.entry_id, group.excluded))
            .collect()
    }

    #[test]
    fn the_list_is_ordered_by_title_without_case_then_by_id() {
        let excluded = titles(&[(7, "round"), (3, "Oval"), (9, "True Cube"), (5, "Oval")]);
        assert_eq!(
            display_order(&excluded),
            vec![(3, "Oval"), (5, "Oval"), (7, "round"), (9, "True Cube")]
        );
        assert_eq!(display_order(&BTreeMap::new()), Vec::<(i32, &str)>::new());
    }

    #[test]
    fn an_id_beyond_the_window_int_is_left_out_of_the_list() {
        let beyond = i64::from(i32::MAX) + 1;
        let excluded = titles(&[(beyond, "Huge"), (2, "Small")]);
        assert_eq!(display_order(&excluded), vec![(2, "Small")]);
    }

    #[test]
    fn the_announcements_name_the_design_and_what_to_do_next() {
        assert_eq!(
            announcement("True Cube", true),
            "Excluded \"True Cube\" from planning. Plan again to see new layouts."
        );
        assert_eq!(
            announcement("True Cube", false),
            "\"True Cube\" can be planned again."
        );
    }

    #[test]
    fn the_row_flags_follow_the_excluded_set_both_ways() {
        let groups = ModelRc::new(VecModel::from(vec![
            group(1, false),
            group(2, true),
            group(3, false),
            group(4, true),
        ]));
        let excluded = BTreeSet::from([1, 4, 99]);
        flag_groups(&groups, &excluded);
        assert_eq!(
            flags(&groups),
            vec![(1, true), (2, false), (3, false), (4, true)]
        );
        // Nothing excluded: every flag clears.
        flag_groups(&groups, &BTreeSet::new());
        assert!(flags(&groups).iter().all(|&(_, flag)| !flag));
    }

    #[test]
    fn a_row_that_already_shows_the_right_flag_is_not_written() {
        use std::cell::Cell;

        /// A model that counts the writes it gets.
        struct Counting {
            inner: VecModel<RoughPlanStoneGroup>,
            writes: Rc<Cell<usize>>,
        }
        impl Model for Counting {
            type Data = RoughPlanStoneGroup;
            fn row_count(&self) -> usize {
                self.inner.row_count()
            }
            fn row_data(&self, row: usize) -> Option<Self::Data> {
                self.inner.row_data(row)
            }
            fn set_row_data(&self, row: usize, data: Self::Data) {
                self.writes.set(self.writes.get() + 1);
                self.inner.set_row_data(row, data);
            }
            fn model_tracker(&self) -> &dyn slint::ModelTracker {
                self.inner.model_tracker()
            }
        }

        let writes = Rc::new(Cell::new(0));
        let groups = ModelRc::new(Counting {
            inner: VecModel::from(vec![group(1, true), group(2, false)]),
            writes: Rc::clone(&writes),
        });
        flag_groups(&groups, &BTreeSet::from([1]));
        assert_eq!(writes.get(), 0, "both rows were right already");
        flag_groups(&groups, &BTreeSet::from([2]));
        assert_eq!(writes.get(), 2, "both rows flipped");
    }
}
