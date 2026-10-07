//! The commands used most recently in this session.
//!
//! Kept in memory only: the list lives as long as the window and is not saved. The palette
//! lists these first when the search box is empty.

use super::search::RECENT_LIMIT;

/// Most recently used command ids, newest first, without repeats.
#[derive(Debug, Default)]
pub struct Recents {
    ids: Vec<&'static str>,
}

impl Recents {
    /// Records that `id` was just used: it moves to the front, and the oldest entry falls
    /// off once [`RECENT_LIMIT`] is reached.
    pub fn record(&mut self, id: &'static str) {
        self.ids.retain(|existing| *existing != id);
        self.ids.insert(0, id);
        self.ids.truncate(RECENT_LIMIT);
    }

    /// The remembered ids, newest first.
    #[must_use]
    pub fn ids(&self) -> &[&'static str] {
        &self.ids
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_newest_command_comes_first() {
        let mut recents = Recents::default();
        recents.record("a");
        recents.record("b");
        recents.record("c");
        assert_eq!(recents.ids(), ["c", "b", "a"]);
    }

    #[test]
    fn using_a_command_again_moves_it_to_the_front_without_repeating_it() {
        let mut recents = Recents::default();
        recents.record("a");
        recents.record("b");
        recents.record("a");
        assert_eq!(recents.ids(), ["a", "b"]);
    }

    #[test]
    fn the_oldest_commands_fall_off_at_the_limit() {
        let mut recents = Recents::default();
        let ids = ["1", "2", "3", "4", "5", "6", "7", "8", "9", "10"];
        for id in ids {
            recents.record(id);
        }
        assert_eq!(recents.ids().len(), RECENT_LIMIT);
        assert_eq!(recents.ids()[0], "10");
        assert!(!recents.ids().contains(&"1"));
        assert!(!recents.ids().contains(&"2"));
    }

    #[test]
    fn a_fresh_list_is_empty() {
        assert_eq!(Recents::default().ids(), Vec::<&str>::new());
    }
}
