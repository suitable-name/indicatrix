use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AngleSetting {
    pub order_index: u32,
    pub facet: String,
    pub angle: String,
    pub index: String,
    pub notes: String,
}
