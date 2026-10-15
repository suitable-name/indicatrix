//! The model behind the app's own numeric keypad.
//!
//! The Android soft keyboard cannot be asked for a numeric layout from Slint, so number
//! entry (an angle, an index position) uses an on-screen keypad of 64 px keys. This is its
//! state: pure, no IME involved. The front end maps each key to a [`Key`] through
//! [`Key::from_id`], calls [`KeypadState::press`], and shows [`KeypadState::display`].

/// The longest entry, in characters, without the sign: `-123.456` fits, nothing longer.
const MAX_CHARS: usize = 7;

/// One key of the keypad.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    /// A digit key, `0` to `9`.
    Digit(u8),
    /// The decimal point.
    Point,
    /// The plus/minus key.
    Sign,
    /// Deletes the last character.
    Backspace,
    /// Clears the entry.
    Clear,
}

impl Key {
    /// The key a Slint key id names: `"0"` to `"9"`, `"."`, `"sign"`, `"back"` or
    /// `"clear"`; `None` for anything else.
    #[must_use]
    pub fn from_id(id: &str) -> Option<Self> {
        match id {
            "." => Some(Self::Point),
            "sign" => Some(Self::Sign),
            "back" => Some(Self::Backspace),
            "clear" => Some(Self::Clear),
            digit => {
                let mut chars = digit.chars();
                let first = chars.next()?;
                if chars.next().is_some() {
                    return None;
                }
                first
                    .to_digit(10)
                    .and_then(|value| u8::try_from(value).ok())
                    .map(Self::Digit)
            }
        }
    }
}

/// What has been typed so far.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct KeypadState {
    /// The digits and at most one decimal point typed so far, without the sign.
    text: String,
    /// Whether the minus sign is on.
    negative: bool,
}

impl KeypadState {
    /// An empty entry.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Applies one key press.
    pub fn press(&mut self, key: Key) {
        match key {
            Key::Digit(digit) if digit <= 9 => self.push_digit(digit),
            Key::Digit(_) => {}
            Key::Point => {
                if !self.text.contains('.') && self.text.len() < MAX_CHARS {
                    if self.text.is_empty() {
                        self.text.push('0');
                    }
                    self.text.push('.');
                }
            }
            Key::Sign => self.negative = !self.negative,
            Key::Backspace => {
                if self.text.pop().is_none() {
                    self.negative = false;
                }
            }
            Key::Clear => {
                self.text.clear();
                self.negative = false;
            }
        }
    }

    fn push_digit(&mut self, digit: u8) {
        if self.text.len() >= MAX_CHARS {
            return;
        }
        let ch = char::from(b'0' + digit);
        if self.text == "0" {
            // No leading zeros: "0" then a digit replaces the zero, "0" then "0" stays.
            if digit != 0 {
                self.text.clear();
                self.text.push(ch);
            }
        } else {
            self.text.push(ch);
        }
    }

    /// Whether nothing has been typed (the sign alone does not count).
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    /// Whether nothing at all has been typed: no digit and no sign.
    #[must_use]
    pub const fn is_pristine(&self) -> bool {
        self.text.is_empty() && !self.negative
    }

    /// Whether the minus sign is on.
    #[must_use]
    pub const fn is_negative(&self) -> bool {
        self.negative
    }

    /// Whether a decimal point has been typed.
    #[must_use]
    pub fn has_point(&self) -> bool {
        self.text.contains('.')
    }

    /// The entry as typed, with its sign: `"-12.5"`, `"7."`, `"0"` when empty, and `"-"`
    /// for a sign with no digits yet.
    #[must_use]
    pub fn display(&self) -> String {
        match (self.text.is_empty(), self.negative) {
            (true, false) => "0".to_owned(),
            (true, true) => "-".to_owned(),
            (false, false) => self.text.clone(),
            (false, true) => format!("-{}", self.text),
        }
    }

    /// The text to hand to a parser: the display, or nothing when no digit was typed.
    /// A negative zero (`"-0"`) is kept, because the sign of a zero angle names its block.
    #[must_use]
    pub fn entry_text(&self) -> Option<String> {
        if self.text.is_empty() {
            None
        } else {
            Some(self.display())
        }
    }

    /// The number typed, `None` when no digit was typed. `"-0"` is minus zero.
    #[must_use]
    pub fn value(&self) -> Option<f64> {
        if self.text.is_empty() {
            return None;
        }
        let magnitude: f64 = self.text.parse().ok()?;
        Some(if self.negative { -magnitude } else { magnitude })
    }
}
