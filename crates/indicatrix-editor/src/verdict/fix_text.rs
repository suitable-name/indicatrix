//! The words a [`FixAction`] shows: the button's label and its tooltip.

use super::FixAction;

impl FixAction {
    /// The button's label.
    #[must_use]
    pub const fn label(&self) -> &'static str {
        match self {
            Self::SnapToTeeth { .. } => "Snap to Teeth",
            Self::RemoveVanished { .. } => "Remove",
            Self::MoveAfter { .. } => "Move Later",
            Self::SteepenPavilion { .. } => "Steepen",
            Self::AddTable => "Add a Table",
        }
    }

    /// What the button does, for its tooltip.
    #[must_use]
    pub const fn hint(&self) -> &'static str {
        match self {
            Self::SnapToTeeth { .. } => {
                "Moves each index position of this tier to the nearest gear tooth."
            }
            Self::RemoveVanished { .. } => {
                "Removes the facets that are cut away. One undo brings them back."
            }
            Self::MoveAfter { .. } => {
                "Moves this tier to just after the tier it meets, so that tier is cut first."
            }
            Self::SteepenPavilion { .. } => {
                "Raises this facet to the critical angle plus 2 degrees. It turns about its \
                 girdle corners, so the girdle outline stays where it is."
            }
            Self::AddTable => {
                "Adds a flat table facet at a height that keeps all the crown facets."
            }
        }
    }
}
