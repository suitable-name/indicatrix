//! [`ImportPreviewChoice`]: what to do about catalogue previews after an import, once
//! the user has asked for the answer to be remembered.

use serde::{Deserialize, Serialize};

/// The remembered answer to "generate preview images for what was just imported?".
///
/// `Ask` (the default) shows the question after every import; the other three are the
/// answers the question's "remember my choice" box stores, applied without asking.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ImportPreviewChoice {
    /// Show the question after every import.
    #[default]
    Ask,
    /// Render the full traced previews right away.
    Full,
    /// Draw fast solid previews right away; the traced ones can follow later.
    Solid,
    /// Generate nothing after an import.
    Skip,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_asks_and_every_variant_round_trips_through_toml() {
        assert_eq!(ImportPreviewChoice::default(), ImportPreviewChoice::Ask);
        for choice in [
            ImportPreviewChoice::Ask,
            ImportPreviewChoice::Full,
            ImportPreviewChoice::Solid,
            ImportPreviewChoice::Skip,
        ] {
            #[derive(Serialize, Deserialize)]
            struct Holder {
                choice: ImportPreviewChoice,
            }
            let text = toml::to_string(&Holder { choice }).unwrap();
            assert_eq!(toml::from_str::<Holder>(&text).unwrap().choice, choice);
        }
    }
}
