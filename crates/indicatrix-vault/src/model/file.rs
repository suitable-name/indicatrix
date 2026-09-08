use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttachedFile {
    pub name: String,
    pub url: String,
    pub content: Vec<u8>,
}
