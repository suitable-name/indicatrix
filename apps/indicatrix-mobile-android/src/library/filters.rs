//! The Library's search text and filters, and their translation to a `LibraryQuery`.
//!
//! The filter sheet shows option lists that start with "Any" (the shapes, the gears and
//! the refractive-index bands) and a tag list; the user's choices are indices into them.
//! [`FilterState`] keeps the choices by value, so a list that changes under it (an import
//! adds a shape) cannot shift a choice onto another entry.

use indicatrix_mobile_core::{
    library::{FilterOptions, LibraryQuery},
    settings::LibrarySort,
};

/// The refractive-index bands of the filter sheet: label, lowest and highest index.
/// Index 0 is "Any".
const RI_BANDS: [(&str, Option<f64>, Option<f64>); 5] = [
    ("Any", None, None),
    ("Below 1.60", None, Some(1.60)),
    ("1.60 to 1.75", Some(1.60), Some(1.75)),
    ("1.75 to 2.00", Some(1.75), Some(2.00)),
    ("Above 2.00", Some(2.00), None),
];

/// The labels of the refractive-index bands.
pub(super) fn ri_labels() -> Vec<String> {
    RI_BANDS
        .iter()
        .map(|(label, _, _)| (*label).to_owned())
        .collect()
}

/// The sort orders in menu order (see [`sort_index`]).
pub(super) const SORT_ORDER: [LibrarySort; 4] = [
    LibrarySort::CatalogueOrder,
    LibrarySort::Title,
    LibrarySort::Newest,
    LibrarySort::RecentlyEdited,
];

/// The menu position of `sort`.
pub(super) fn sort_index(sort: LibrarySort) -> i32 {
    SORT_ORDER
        .iter()
        .position(|candidate| *candidate == sort)
        .map_or(0, index_i32)
}

/// The sort at menu position `index`, `None` for a position outside the menu.
pub(super) fn sort_from_index(index: i32) -> Option<LibrarySort> {
    usize::try_from(index)
        .ok()
        .and_then(|index| SORT_ORDER.get(index).copied())
}

/// A list position as the UI's `int`.
pub(super) fn index_i32(index: usize) -> i32 {
    i32::try_from(index).unwrap_or(i32::MAX)
}

/// The search text and the filters. `Default` is "show everything that is not ignored".
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct FilterState {
    /// The text in the search box.
    pub text: String,
    /// Only this shape.
    pub shape: Option<String>,
    /// Only this index gear.
    pub gear: Option<u32>,
    /// The refractive-index band (0 is any).
    pub ri_band: usize,
    /// Only designs the user made or saved.
    pub mine_only: bool,
    /// Only designs with this tag.
    pub tag: Option<String>,
    /// Include designs marked as ignored.
    pub show_ignored: bool,
}

impl FilterState {
    /// The query for the current choices, in `sort` order.
    pub fn to_query(&self, sort: LibrarySort) -> LibraryQuery {
        let (_, ri_min, ri_max) = RI_BANDS.get(self.ri_band).copied().unwrap_or(RI_BANDS[0]);
        LibraryQuery {
            text: self.text.trim().to_owned(),
            shape: self.shape.clone(),
            gear: self.gear,
            ri_min,
            ri_max,
            mine_only: self.mine_only,
            tag: self.tag.clone(),
            show_ignored: self.show_ignored,
            sort,
        }
    }

    /// How many filters are set. The search text is not a filter.
    pub fn active_count(&self) -> usize {
        usize::from(self.shape.is_some())
            + usize::from(self.gear.is_some())
            + usize::from(self.ri_band > 0)
            + usize::from(self.mine_only)
            + usize::from(self.tag.is_some())
            + usize::from(self.show_ignored)
    }

    /// Whether the search text or any filter narrows the list.
    pub fn is_narrowing(&self) -> bool {
        !self.text.trim().is_empty() || self.active_count() > 0
    }

    /// Clears the search text and the filters.
    pub fn clear_all(&mut self) {
        *self = Self::default();
    }

    /// Drops the choices that `options` no longer offers (the last design of a shape
    /// was deleted). Returns true when something was dropped.
    pub fn prune(&mut self, options: &FilterOptions) -> bool {
        let drop_shape = self
            .shape
            .as_ref()
            .is_some_and(|shape| !options.shapes.contains(shape));
        let drop_gear = self.gear.is_some_and(|gear| !options.gears.contains(&gear));
        let drop_tag = self
            .tag
            .as_ref()
            .is_some_and(|tag| !options.tags.contains(tag));
        if drop_shape {
            self.shape = None;
        }
        if drop_gear {
            self.gear = None;
        }
        if drop_tag {
            self.tag = None;
        }
        drop_shape || drop_gear || drop_tag
    }

    /// The chosen shape's position in the shape chips ("Any" is 0).
    pub fn shape_index(&self, options: &FilterOptions) -> i32 {
        self.shape
            .as_ref()
            .and_then(|shape| options.shapes.iter().position(|other| other == shape))
            .map_or(0, |position| index_i32(position + 1))
    }

    /// Chooses the shape at chip position `index` ("Any" or an unknown position clears).
    pub fn pick_shape(&mut self, options: &FilterOptions, index: i32) {
        self.shape =
            chip_position(index).and_then(|position| options.shapes.get(position).cloned());
    }

    /// The chosen gear's position in the gear chips ("Any" is 0).
    pub fn gear_index(&self, options: &FilterOptions) -> i32 {
        self.gear
            .and_then(|gear| options.gears.iter().position(|other| *other == gear))
            .map_or(0, |position| index_i32(position + 1))
    }

    /// Chooses the gear at chip position `index`.
    pub fn pick_gear(&mut self, options: &FilterOptions, index: i32) {
        self.gear = chip_position(index).and_then(|position| options.gears.get(position).copied());
    }

    /// Chooses the refractive-index band at chip position `index`.
    pub fn pick_ri(&mut self, index: i32) {
        self.ri_band = usize::try_from(index)
            .ok()
            .filter(|band| *band < RI_BANDS.len())
            .unwrap_or(0);
    }

    /// The chosen tag's position in the tag list, -1 for none.
    pub fn tag_index(&self, options: &FilterOptions) -> i32 {
        self.tag
            .as_ref()
            .and_then(|tag| options.tags.iter().position(|other| other == tag))
            .map_or(-1, index_i32)
    }

    /// Chooses the tag at list position `index` (a negative position clears).
    pub fn pick_tag(&mut self, options: &FilterOptions, index: i32) {
        self.tag = usize::try_from(index)
            .ok()
            .and_then(|position| options.tags.get(position).cloned());
    }
}

/// The option-list position of chip `index`, where chip 0 is "Any".
fn chip_position(index: i32) -> Option<usize> {
    usize::try_from(index).ok()?.checked_sub(1)
}

/// The shape chips: "Any" first, then the shapes.
pub(super) fn shape_labels(options: &FilterOptions) -> Vec<String> {
    std::iter::once("Any".to_owned())
        .chain(
            options
                .shapes
                .iter()
                .map(|shape| super::format::capitalize(shape)),
        )
        .collect()
}

/// The gear chips: "Any" first, then the gears.
pub(super) fn gear_labels(options: &FilterOptions) -> Vec<String> {
    std::iter::once("Any".to_owned())
        .chain(options.gears.iter().map(u32::to_string))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn options() -> FilterOptions {
        FilterOptions {
            shapes: vec!["round".to_owned(), "oval".to_owned()],
            gears: vec![64, 96],
            tags: vec!["fantasy".to_owned(), "mine".to_owned(), "test".to_owned()],
        }
    }

    #[test]
    fn default_query_is_open() {
        let query = FilterState::default().to_query(LibrarySort::Title);
        assert_eq!(query.text, "");
        assert_eq!(query.shape, None);
        assert_eq!(query.ri_min, None);
        assert_eq!(query.ri_max, None);
        assert!(!query.show_ignored);
        assert_eq!(query.sort, LibrarySort::Title);
    }

    #[test]
    fn query_carries_every_choice() {
        let options = options();
        let mut state = FilterState {
            text: "  barion ".to_owned(),
            mine_only: true,
            show_ignored: true,
            ..FilterState::default()
        };
        state.pick_shape(&options, 2);
        state.pick_gear(&options, 2);
        state.pick_ri(2);
        state.pick_tag(&options, 1);
        let query = state.to_query(LibrarySort::Newest);
        assert_eq!(query.text, "barion");
        assert_eq!(query.shape.as_deref(), Some("oval"));
        assert_eq!(query.gear, Some(96));
        assert_eq!(query.ri_min, Some(1.60));
        assert_eq!(query.ri_max, Some(1.75));
        assert_eq!(query.tag.as_deref(), Some("mine"));
        assert!(query.mine_only);
        assert!(query.show_ignored);
        assert_eq!(state.active_count(), 6);
    }

    #[test]
    fn any_and_unknown_positions_clear() {
        let options = options();
        let mut state = FilterState::default();
        state.pick_shape(&options, 1);
        assert_eq!(state.shape_index(&options), 1);
        state.pick_shape(&options, 0);
        assert_eq!(state.shape, None);
        state.pick_shape(&options, 9);
        assert_eq!(state.shape, None);
        state.pick_tag(&options, 2);
        assert_eq!(state.tag_index(&options), 2);
        state.pick_tag(&options, -1);
        assert_eq!(state.tag_index(&options), -1);
        state.pick_ri(99);
        assert_eq!(state.ri_band, 0);
    }

    #[test]
    fn clearing_drops_the_text_and_the_filters() {
        let mut state = FilterState {
            text: "emerald".to_owned(),
            mine_only: true,
            ..FilterState::default()
        };
        assert!(state.is_narrowing());
        state.clear_all();
        assert_eq!(state, FilterState::default());
        assert!(!state.is_narrowing());
        // Text alone narrows too, but is not a filter.
        state.text = "ruby".to_owned();
        assert!(state.is_narrowing());
        assert_eq!(state.active_count(), 0);
    }

    #[test]
    fn prune_drops_vanished_choices() {
        let mut state = FilterState {
            shape: Some("heart".to_owned()),
            gear: Some(96),
            tag: Some("gone".to_owned()),
            ..FilterState::default()
        };
        assert!(state.prune(&options()));
        assert_eq!(state.shape, None);
        assert_eq!(state.gear, Some(96));
        assert_eq!(state.tag, None);
        assert!(!state.prune(&options()));
    }

    #[test]
    fn labels_start_with_any() {
        let options = options();
        assert_eq!(shape_labels(&options), ["Any", "Round", "Oval"]);
        assert_eq!(gear_labels(&options), ["Any", "64", "96"]);
        assert_eq!(ri_labels()[0], "Any");
        assert_eq!(ri_labels().len(), RI_BANDS.len());
    }

    #[test]
    fn sort_menu_round_trips() {
        for (index, sort) in SORT_ORDER.iter().enumerate() {
            assert_eq!(sort_index(*sort), index_i32(index));
            assert_eq!(sort_from_index(index_i32(index)), Some(*sort));
        }
        assert_eq!(sort_from_index(-1), None);
        assert_eq!(sort_from_index(4), None);
    }
}
