//! The tag sheet's logic: which tags to add to and remove from the selected designs.
//!
//! `LibraryHandle::set_tags` replaces the tags of the designs it is given with one list,
//! so changing tags on designs that carry different tags needs one call per distinct
//! result. [`plan`] works that out.

use std::collections::BTreeMap;

use indicatrix_mobile_core::library::EntryId;

/// The longest tag name kept.
const MAX_TAG_LEN: usize = 40;

/// A tag name as stored: whitespace collapsed, at most 40 characters. `None` when the
/// name is blank.
pub(super) fn normalize(name: &str) -> Option<String> {
    let collapsed = name.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.is_empty() {
        None
    } else {
        Some(collapsed.chars().take(MAX_TAG_LEN).collect())
    }
}

/// Whether two tag names are the same tag (case does not matter).
fn same(a: &str, b: &str) -> bool {
    a.to_lowercase() == b.to_lowercase()
}

/// One `set_tags` call: the designs and the complete tag list they end up with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Change {
    /// The designs that get `tags`.
    pub ids: Vec<EntryId>,
    /// Their new tags.
    pub tags: Vec<String>,
}

/// The `set_tags` calls that add `add` to and remove `remove` from every design of
/// `cards` (id and current tags). Designs whose tags would not change are left out.
pub(super) fn plan(
    cards: &[(EntryId, Vec<String>)],
    add: &[String],
    remove: &[String],
) -> Vec<Change> {
    let mut groups: BTreeMap<Vec<String>, Vec<EntryId>> = BTreeMap::new();
    for (id, current) in cards {
        let mut next: Vec<String> = current
            .iter()
            .filter(|tag| !remove.iter().any(|gone| same(gone, tag)))
            .cloned()
            .collect();
        for tag in add {
            if !next.iter().any(|have| same(have, tag)) {
                next.push(tag.clone());
            }
        }
        if next != *current {
            groups.entry(next).or_default().push(*id);
        }
    }
    groups
        .into_iter()
        .map(|(tags, ids)| Change { ids, tags })
        .collect()
}

/// A chip of the tag sheet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Chip {
    /// The tag.
    pub text: String,
    /// Whether it is chosen.
    pub on: bool,
}

/// The tag sheet while it is open: the tags to add (every tag of the library, plus new
/// ones) and the tags to remove (those the selected designs carry).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct SheetState {
    /// The "Add Tags" chips.
    pub add: Vec<Chip>,
    /// The "Remove Tags" chips.
    pub remove: Vec<Chip>,
}

impl SheetState {
    /// A sheet for designs that carry `carried` tags, in a library that has `library`
    /// tags. Nothing is chosen yet.
    pub fn open(library: &[String], carried: &[Vec<String>]) -> Self {
        let mut remove: Vec<Chip> = Vec::new();
        for tag in carried.iter().flatten() {
            if !remove.iter().any(|chip| same(&chip.text, tag)) {
                remove.push(Chip {
                    text: tag.clone(),
                    on: false,
                });
            }
        }
        remove.sort_by_key(|chip| chip.text.to_lowercase());
        Self {
            add: library
                .iter()
                .map(|tag| Chip {
                    text: tag.clone(),
                    on: false,
                })
                .collect(),
            remove,
        }
    }

    /// Flips the "add" chip at `index`. A tag cannot be added and removed at once.
    pub fn toggle_add(&mut self, index: usize) {
        let Some(chip) = self.add.get_mut(index) else {
            return;
        };
        chip.on = !chip.on;
        if chip.on {
            let name = chip.text.clone();
            for other in self
                .remove
                .iter_mut()
                .filter(|other| same(&other.text, &name))
            {
                other.on = false;
            }
        }
    }

    /// Flips the "remove" chip at `index`.
    pub fn toggle_remove(&mut self, index: usize) {
        let Some(chip) = self.remove.get_mut(index) else {
            return;
        };
        chip.on = !chip.on;
        if chip.on {
            let name = chip.text.clone();
            for other in self.add.iter_mut().filter(|other| same(&other.text, &name)) {
                other.on = false;
            }
        }
    }

    /// Adds a typed tag name as a chosen "add" chip (or chooses the chip it matches).
    /// Returns false for a blank name.
    pub fn add_new(&mut self, name: &str) -> bool {
        let Some(name) = normalize(name) else {
            return false;
        };
        if let Some(position) = self.add.iter().position(|chip| same(&chip.text, &name)) {
            if !self.add[position].on {
                self.toggle_add(position);
            }
        } else {
            self.add.push(Chip {
                text: name.clone(),
                on: true,
            });
            for other in self
                .remove
                .iter_mut()
                .filter(|other| same(&other.text, &name))
            {
                other.on = false;
            }
        }
        true
    }

    /// The chosen tags to add.
    pub fn chosen_add(&self) -> Vec<String> {
        self.add
            .iter()
            .filter(|chip| chip.on)
            .map(|chip| chip.text.clone())
            .collect()
    }

    /// The chosen tags to remove.
    pub fn chosen_remove(&self) -> Vec<String> {
        self.remove
            .iter()
            .filter(|chip| chip.on)
            .map(|chip| chip.text.clone())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tags(names: &[&str]) -> Vec<String> {
        names.iter().map(|name| (*name).to_owned()).collect()
    }

    #[test]
    fn normalize_collapses_and_limits() {
        assert_eq!(normalize("  fancy   cut "), Some("fancy cut".to_owned()));
        assert_eq!(normalize("   "), None);
        assert_eq!(normalize(&"x".repeat(60)).map(|tag| tag.len()), Some(40));
    }

    #[test]
    fn plan_groups_designs_by_result() {
        let cards = vec![
            (1, tags(&["a"])),
            (2, tags(&["a"])),
            (3, tags(&["b"])),
            (4, tags(&["new"])),
        ];
        let changes = plan(&cards, &tags(&["new"]), &[]);
        assert_eq!(
            changes,
            vec![
                Change {
                    ids: vec![1, 2],
                    tags: tags(&["a", "new"])
                },
                Change {
                    ids: vec![3],
                    tags: tags(&["b", "new"])
                },
            ]
        );
    }

    #[test]
    fn plan_removes_and_ignores_case() {
        let cards = vec![(1, tags(&["Gem", "x"])), (2, tags(&["y"]))];
        let changes = plan(&cards, &[], &tags(&["gem"]));
        assert_eq!(
            changes,
            vec![Change {
                ids: vec![1],
                tags: tags(&["x"])
            }]
        );
        // Adding a tag that is already there (in another case) changes nothing.
        assert_eq!(plan(&cards[..1], &tags(&["GEM"]), &[]).len(), 0);
    }

    #[test]
    fn plan_can_clear_all_tags() {
        let cards = vec![(7, tags(&["only"]))];
        let changes = plan(&cards, &[], &tags(&["only"]));
        assert_eq!(
            changes,
            vec![Change {
                ids: vec![7],
                tags: Vec::new()
            }]
        );
    }

    #[test]
    fn sheet_lists_library_and_carried_tags() {
        let sheet = SheetState::open(&tags(&["a", "b"]), &[tags(&["b", "z"]), tags(&["B", "c"])]);
        let add: Vec<&str> = sheet.add.iter().map(|chip| chip.text.as_str()).collect();
        let remove: Vec<&str> = sheet.remove.iter().map(|chip| chip.text.as_str()).collect();
        assert_eq!(add, ["a", "b"]);
        assert_eq!(remove, ["b", "c", "z"]);
        assert_eq!(sheet.chosen_add().len(), 0);
        assert_eq!(sheet.chosen_remove().len(), 0);
    }

    #[test]
    fn a_tag_is_never_added_and_removed_at_once() {
        let mut sheet = SheetState::open(&tags(&["a"]), &[tags(&["a"])]);
        sheet.toggle_remove(0);
        assert_eq!(sheet.chosen_remove(), ["a"]);
        sheet.toggle_add(0);
        assert_eq!(sheet.chosen_add(), ["a"]);
        assert_eq!(sheet.chosen_remove().len(), 0);
        sheet.toggle_remove(0);
        assert_eq!(sheet.chosen_add().len(), 0);
        assert_eq!(sheet.chosen_remove(), ["a"]);
    }

    #[test]
    fn typed_tags_become_chosen_chips() {
        let mut sheet = SheetState::open(&tags(&["a"]), &[]);
        assert!(!sheet.add_new("  "));
        assert!(sheet.add_new("  Rare  "));
        assert!(sheet.add_new("A"));
        assert_eq!(sheet.chosen_add(), ["a", "Rare"]);
        // Typing a name again does not switch it off.
        assert!(sheet.add_new("rare"));
        assert_eq!(sheet.chosen_add(), ["a", "Rare"]);
        // Out-of-range toggles are ignored.
        sheet.toggle_add(99);
        sheet.toggle_remove(99);
    }
}
