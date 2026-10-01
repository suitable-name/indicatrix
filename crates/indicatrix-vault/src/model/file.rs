use serde::{Deserialize, Serialize};

/// One attachment WITH its full byte content.
///
/// A `.asc`/`.gem`/`.pdf`/native-sidecar file, or a scraped page's attachment -- see
/// [`crate::model::entry::AttachedFileMeta`] for the metadata-only counterpart a
/// caller that doesn't need the bytes should prefer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttachedFile {
    /// The attachment's file name, e.g. `"design.asc"`.
    pub name: String,
    /// The source page's URL for this attachment, or empty for a locally-imported
    /// file that never had one (see `crate::local::import_asc`).
    pub url: String,
    /// The attachment's full byte content.
    pub content: Vec<u8>,
}
