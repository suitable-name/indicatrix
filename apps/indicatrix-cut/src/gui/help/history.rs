//! The viewer's Back and Forward trail.

/// How many places the trail remembers.
const LIMIT: usize = 100;

/// The places visited, with a cursor, like a browser's history.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Trail<T> {
    back: Vec<T>,
    forward: Vec<T>,
    current: Option<T>,
}

impl<T> Default for Trail<T> {
    fn default() -> Self {
        Self {
            back: Vec::new(),
            forward: Vec::new(),
            current: None,
        }
    }
}

impl<T: Clone + PartialEq> Trail<T> {
    /// Goes to `place`. The place left becomes the Back target and the Forward trail is
    /// dropped. Visiting the place already shown changes nothing.
    pub fn visit(&mut self, place: T) {
        if self.current.as_ref() == Some(&place) {
            return;
        }
        if let Some(left) = self.current.replace(place) {
            self.back.push(left);
            if self.back.len() > LIMIT {
                self.back.remove(0);
            }
        }
        self.forward.clear();
    }

    /// Steps back; returns the place now shown, or `None` at the start of the trail.
    pub fn go_back(&mut self) -> Option<T> {
        let place = self.back.pop()?;
        if let Some(left) = self.current.replace(place.clone()) {
            self.forward.push(left);
        }
        Some(place)
    }

    /// Steps forward; returns the place now shown, or `None` at the end of the trail.
    pub fn go_forward(&mut self) -> Option<T> {
        let place = self.forward.pop()?;
        if let Some(left) = self.current.replace(place.clone()) {
            self.back.push(left);
        }
        Some(place)
    }

    /// Whether there is a place to go back to.
    #[must_use]
    pub const fn can_go_back(&self) -> bool {
        !self.back.is_empty()
    }

    /// Whether there is a place to go forward to.
    #[must_use]
    pub const fn can_go_forward(&self) -> bool {
        !self.forward.is_empty()
    }

    /// The place shown now.
    #[must_use]
    pub const fn current(&self) -> Option<&T> {
        self.current.as_ref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fresh_trail_goes_nowhere() {
        let mut trail: Trail<u32> = Trail::default();
        assert!(!trail.can_go_back() && !trail.can_go_forward());
        assert_eq!(trail.go_back(), None);
        assert_eq!(trail.go_forward(), None);
        assert_eq!(trail.current(), None);
    }

    #[test]
    fn back_and_forward_walk_the_visits() {
        let mut trail = Trail::default();
        for place in [1, 2, 3] {
            trail.visit(place);
        }
        assert_eq!(trail.current(), Some(&3));
        assert_eq!(trail.go_back(), Some(2));
        assert_eq!(trail.go_back(), Some(1));
        assert!(!trail.can_go_back() && trail.can_go_forward());
        assert_eq!(trail.go_forward(), Some(2));
        assert_eq!(trail.current(), Some(&2));
    }

    #[test]
    fn a_new_visit_drops_the_forward_trail() {
        let mut trail = Trail::default();
        for place in [1, 2, 3] {
            trail.visit(place);
        }
        trail.go_back();
        trail.visit(9);
        assert!(!trail.can_go_forward());
        // Back from 9 returns to the place left, 2; the abandoned 3 is gone.
        assert_eq!(trail.go_back(), Some(2));
        assert_eq!(trail.go_back(), Some(1));
    }

    #[test]
    fn visiting_the_place_already_shown_changes_nothing() {
        let mut trail = Trail::default();
        trail.visit(1);
        trail.visit(1);
        assert!(!trail.can_go_back());
        trail.visit(2);
        trail.visit(2);
        assert_eq!(trail.go_back(), Some(1));
        assert_eq!(trail.go_back(), None);
    }

    #[test]
    fn the_trail_keeps_only_the_latest_places() {
        let mut trail = Trail::default();
        for place in 0..(LIMIT as u32 + 20) {
            trail.visit(place);
        }
        let mut steps = 0;
        while trail.go_back().is_some() {
            steps += 1;
        }
        assert_eq!(steps, LIMIT);
    }
}
