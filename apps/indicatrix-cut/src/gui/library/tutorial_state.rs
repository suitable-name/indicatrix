//! What the library tutorials wait for: a state, not "something changed".
//!
//! "Clear it" asks for an empty search box, and "Put everything back" for a library that no
//! search or filter narrows any more. Reporting "the search was edited" or "a filter changed"
//! would finish those steps when the learner types one more letter or picks a second shape, so
//! the library handlers ask [`library_is_narrowed`] after every change and report the reset only
//! when [`NarrowingMemory`] sees the library go from narrowed to not narrowed.

use super::search::{read_local_only, read_range_filter, read_tag_filter};
use crate::MainWindow;
use indicatrix_editor::guide::viewing_events as events;
use indicatrix_vault::model::filter::RangeFilter;
use std::cell::Cell;

/// The library's search and filters, as far as they narrow what the list shows.
#[derive(Clone, Copy, Debug)]
pub(super) struct Narrowing<'a> {
    /// The search box.
    pub(super) search: &'a str,
    /// The Shape drop-down's text ("All Shapes" for no filter).
    pub(super) shape: &'a str,
    /// The Gear drop-down's text ("All Gears" for no filter).
    pub(super) gear: &'a str,
    /// The numeric range filters, the tilt-performance rows and "Show ignored".
    pub(super) range: &'a RangeFilter,
    /// Only the designs of the local library.
    pub(super) local_only: bool,
    /// A tag chip is chosen.
    pub(super) tag_chosen: bool,
}

impl Narrowing<'_> {
    /// Whether any of them narrows the list. The sort order is not a filter, and "Show
    /// ignored" only widens the list, so neither counts.
    pub(super) fn is_on(&self) -> bool {
        let narrowing_ranges = RangeFilter {
            include_ignored: false,
            ..self.range.clone()
        };
        !self.search.trim().is_empty()
            || !matches!(self.shape, "All Shapes" | "All" | "")
            || !matches!(self.gear, "All Gears" | "All" | "")
            || self.local_only
            || self.tag_chosen
            || narrowing_ranges != RangeFilter::default()
    }
}

/// Whether the library's search or a filter narrows the list now: the search box and the Shape
/// and Gear drop-downs as the caller has them (a handler passes the value it was just given,
/// because the model property may not have caught up with it yet), and the range, "My designs"
/// and tag filters read off the window (UI thread only).
pub(super) fn library_is_narrowed(ui: &MainWindow, search: &str, shape: &str, gear: &str) -> bool {
    let range = read_range_filter(ui);
    Narrowing {
        search,
        shape,
        gear,
        range: &range,
        local_only: read_local_only(ui),
        tag_chosen: read_tag_filter(ui).is_some(),
    }
    .is_on()
}

/// The tutorial event for an edit of the search box that left `search` in it: the search box
/// holds text, or it was emptied.
pub(super) fn search_event(search: &str) -> &'static str {
    if search.trim().is_empty() {
        events::LIBRARY_SEARCH_CLEARED
    } else {
        events::LIBRARY_SEARCHED
    }
}

/// Remembers whether the library was narrowed after the last change, so that taking the last
/// search or filter off can be told from changing one.
#[derive(Debug, Default)]
pub(super) struct NarrowingMemory {
    narrowed: Cell<bool>,
}

impl NarrowingMemory {
    /// Records whether the library is narrowed now. Returns `true` exactly when it was
    /// narrowed after the last change and is not any more.
    pub(super) const fn went_off(&self, narrowed_now: bool) -> bool {
        self.narrowed.replace(narrowed_now) && !narrowed_now
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn untouched(range: &RangeFilter) -> Narrowing<'_> {
        Narrowing {
            search: "",
            shape: "All Shapes",
            gear: "All Gears",
            range,
            local_only: false,
            tag_chosen: false,
        }
    }

    #[test]
    fn an_untouched_library_is_not_narrowed() {
        let range = RangeFilter::default();
        assert!(!untouched(&range).is_on());
    }

    #[test]
    fn a_search_a_shape_a_gear_my_designs_a_tag_or_a_range_narrows_it() {
        let range = RangeFilter::default();
        let base = untouched(&range);
        assert!(
            Narrowing {
                search: "rbc",
                ..base
            }
            .is_on()
        );
        assert!(
            Narrowing {
                shape: "Round",
                ..base
            }
            .is_on()
        );
        assert!(Narrowing { gear: "96", ..base }.is_on());
        assert!(
            Narrowing {
                local_only: true,
                ..base
            }
            .is_on()
        );
        assert!(
            Narrowing {
                tag_chosen: true,
                ..base
            }
            .is_on()
        );
        let narrowed = RangeFilter {
            ri_min: Some(1.6),
            ..RangeFilter::default()
        };
        assert!(untouched(&narrowed).is_on());
    }

    #[test]
    fn spaces_in_the_box_and_the_all_entries_are_no_filter() {
        let range = RangeFilter::default();
        let base = untouched(&range);
        assert!(
            !Narrowing {
                search: "   ",
                shape: "All",
                gear: "All",
                ..base
            }
            .is_on()
        );
    }

    #[test]
    fn show_ignored_only_widens_the_list() {
        let range = RangeFilter {
            include_ignored: true,
            ..RangeFilter::default()
        };
        assert!(!untouched(&range).is_on());
    }

    #[test]
    fn an_emptied_box_reports_a_cleared_search_and_a_box_with_text_a_search() {
        assert_eq!(search_event("rbc"), events::LIBRARY_SEARCHED);
        assert_eq!(search_event(" r "), events::LIBRARY_SEARCHED);
        assert_eq!(search_event(""), events::LIBRARY_SEARCH_CLEARED);
        assert_eq!(search_event("  "), events::LIBRARY_SEARCH_CLEARED);
    }

    #[test]
    fn the_memory_reports_only_the_change_from_narrowed_to_not() {
        let memory = NarrowingMemory::default();
        // A change while nothing is narrowed (the sort order) is not a reset.
        assert!(!memory.went_off(false));
        // Narrowing, and narrowing further, is not a reset either.
        assert!(!memory.went_off(true));
        assert!(!memory.went_off(true));
        // Taking the last one off is, once.
        assert!(memory.went_off(false));
        assert!(!memory.went_off(false));
        // And again after the library is narrowed anew.
        assert!(!memory.went_off(true));
        assert!(memory.went_off(false));
    }
}
