//! The state behind the number keypad that every number field of the Stone and Checks tabs
//! (and the New Design form) opens.
//!
//! A [`NumberEntry`] wraps the shared [`KeypadState`] with the value the field held when the
//! keypad opened: until the first key, the readout shows that value; after it, what has been
//! typed. The front end maps every key to its id, calls [`NumberEntry::press`] and shows the
//! returned [`NumberView`].

use crate::design_state::{Key, KeypadState};

/// What the keypad's readout and the commit button need after a key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NumberView {
    /// The text the readout shows: the field's value, or what has been typed.
    pub readout: String,
    /// Whether the readout shows typed text (and so a commit would change the field).
    pub typing: bool,
    /// The text to commit; empty when nothing usable was typed.
    pub text: String,
}

/// The keypad's typed text together with the value it started from.
#[derive(Debug, Clone, Default)]
pub struct NumberEntry {
    keypad: KeypadState,
    current: String,
}

impl NumberEntry {
    /// An entry with nothing typed and no starting value.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Starts a new entry for a field that holds `current` (empty for "not set").
    pub fn begin(&mut self, current: &str) -> NumberView {
        self.keypad = KeypadState::new();
        current.trim().clone_into(&mut self.current);
        self.view()
    }

    /// Applies the key named by `id` (`"0"` to `"9"`, `"."`, `"sign"`, `"back"`, `"clear"`;
    /// anything else is ignored).
    pub fn press(&mut self, id: &str) -> NumberView {
        if let Some(key) = Key::from_id(id) {
            self.keypad.press(key);
        }
        self.view()
    }

    /// The readout, the typing flag and the commit text for the entry as it stands.
    #[must_use]
    pub fn view(&self) -> NumberView {
        if self.keypad.is_pristine() {
            NumberView {
                readout: if self.current.is_empty() {
                    "0".to_owned()
                } else {
                    self.current.clone()
                },
                typing: false,
                text: String::new(),
            }
        } else {
            NumberView {
                readout: self.keypad.display(),
                typing: true,
                text: self.keypad.entry_text().unwrap_or_default(),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_readout_starts_at_the_fields_value() {
        let mut entry = NumberEntry::new();
        let view = entry.begin(" 7.5 ");
        assert_eq!(view.readout, "7.5");
        assert!(!view.typing);
        assert_eq!(view.text.len(), 0);
        assert_eq!(entry.begin("").readout, "0");
    }

    #[test]
    fn typing_replaces_the_value_and_yields_the_commit_text() {
        let mut entry = NumberEntry::new();
        entry.begin("7.5");
        entry.press("6");
        let view = entry.press(".");
        assert_eq!(view.readout, "6.");
        assert!(view.typing);
        assert_eq!(view.text, "6.");
        let view = entry.press("2");
        assert_eq!((view.readout.as_str(), view.text.as_str()), ("6.2", "6.2"));
    }

    #[test]
    fn a_sign_alone_commits_nothing() {
        let mut entry = NumberEntry::new();
        entry.begin("1.5");
        let view = entry.press("sign");
        assert_eq!(view.readout, "-");
        assert!(view.typing);
        assert_eq!(view.text.len(), 0);
        let view = entry.press("4");
        assert_eq!(view.text, "-4");
    }

    #[test]
    fn deleting_everything_returns_to_the_value() {
        let mut entry = NumberEntry::new();
        entry.begin("2.5");
        entry.press("9");
        let view = entry.press("back");
        assert!(!view.typing, "the typed digit is gone");
        assert_eq!(view.readout, "2.5");
        let view = entry.press("clear");
        assert_eq!(view.readout, "2.5");
    }

    #[test]
    fn unknown_keys_are_ignored_and_begin_forgets_what_was_typed() {
        let mut entry = NumberEntry::new();
        entry.begin("1");
        entry.press("3");
        assert_eq!(entry.press("nonsense").readout, "3");
        let view = entry.begin("1");
        assert_eq!(view.readout, "1");
        assert!(!view.typing);
    }
}
