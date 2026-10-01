//! The New Design template gallery's cards.
//!
//! The template data itself lives in
//! `indicatrix_cut_core::templates` (five entries, each a statically verified
//! closed solid); this module lists them as gallery cards, with index 0 "Empty"
//! first -- the same index [`crate::EditorSession::from_template`] takes, so card
//! `i` creates template `i`.

/// One gallery card.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TemplateCard {
    /// The card's title and the "Start From" selection text.
    pub name: String,
    /// A short shape/fold line under the name.
    pub shape: String,
    /// One sentence describing the design.
    pub description: String,
    /// Whether the card can be selected (every card can today).
    pub ready: bool,
}

/// Every gallery card, in display order: "Empty" first, then every
/// `indicatrix_cut_core::templates::TEMPLATES` entry in order -- every one of them
/// selectable.
#[must_use]
pub fn template_cards() -> Vec<TemplateCard> {
    let mut cards = vec![TemplateCard {
        name: "Empty".to_string(),
        shape: "Blank design".to_string(),
        description: "No starting tiers -- author the schedule from scratch.".to_string(),
        ready: true,
    }];
    for spec in indicatrix_cut_core::templates::TEMPLATES {
        cards.push(TemplateCard {
            name: spec.name.to_string(),
            shape: spec.shape.to_string(),
            description: spec.description.to_string(),
            ready: true,
        });
    }
    cards
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_template_gets_a_card_after_empty() {
        let cards = template_cards();
        assert_eq!(
            cards.len(),
            indicatrix_cut_core::templates::TEMPLATES.len() + 1
        );
        assert_eq!(cards[0].name, "Empty");
        assert!(cards.iter().all(|card| card.ready));
        assert_eq!(
            cards[1].name,
            indicatrix_cut_core::templates::TEMPLATES[0].name
        );
    }
}
