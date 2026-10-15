//! The two facts about the open design that belong to ONE browser tab, kept apart from the
//! shared settings.
//!
//! The render settings live in `localStorage`, which every tab of the page shares. The
//! design's recorded `.asc` name and its unsaved flag describe the design THIS tab has open,
//! so they are stored in this tab's `sessionStorage` as a [`TabState`]. A payload an
//! earlier build wrote carries the same two facts inside [`SessionPayload`]; they are only
//! read as a fallback ([`TabState::from_payload`]) when the tab has no state of its own.

use crate::settings::SessionPayload;
use serde::{Deserialize, Serialize};

/// What `sessionStorage["indicatrix.tab.v1"]` holds.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct TabState {
    /// The design's recorded `.asc` name (`None` for a design never saved).
    pub design_name: Option<String>,
    /// Whether the design had unsaved changes.
    pub design_unsaved: bool,
}

impl TabState {
    /// The state an earlier build kept inside the shared settings payload.
    #[must_use]
    pub fn from_payload(payload: &SessionPayload) -> Self {
        Self {
            design_name: payload.design_name.clone(),
            design_unsaved: payload.design_unsaved,
        }
    }

    /// Parses a stored tab state.
    ///
    /// # Errors
    ///
    /// The JSON parser's message.
    pub fn from_json(text: &str) -> Result<Self, String> {
        serde_json::from_str(text).map_err(|e| e.to_string())
    }

    /// The JSON to store.
    ///
    /// # Errors
    ///
    /// The serializer's message (in practice unreachable for these types).
    pub fn to_json(&self) -> Result<String, String> {
        serde_json::to_string(self).map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tab_state_round_trips() {
        let state = TabState {
            design_name: Some("round.asc".to_string()),
            design_unsaved: true,
        };
        let json = state.to_json().expect("serializes");
        assert_eq!(TabState::from_json(&json), Ok(state));
    }

    #[test]
    fn missing_fields_take_their_defaults() {
        assert_eq!(TabState::from_json("{}"), Ok(TabState::default()));
        assert!(TabState::from_json("not json").is_err());
    }

    #[test]
    fn an_old_payload_supplies_the_fallback() {
        let payload = SessionPayload {
            design_name: Some("old.asc".to_string()),
            design_unsaved: true,
            ..SessionPayload::default()
        };
        let state = TabState::from_payload(&payload);
        assert_eq!(state.design_name.as_deref(), Some("old.asc"));
        assert!(state.design_unsaved);
    }

    #[test]
    fn an_old_payload_with_the_fields_still_loads() {
        let payload = SessionPayload::from_json(r#"{"design_name":"x.asc","design_unsaved":true}"#)
            .expect("old payloads load");
        assert_eq!(
            TabState::from_payload(&payload).design_name.as_deref(),
            Some("x.asc")
        );
    }
}
