//! What a `.indicatrix` design file carries besides the design itself, kept for the
//! open design so the next Save writes it back unchanged: the `[meta]` table (with its
//! unknown keys) and the byte-exact `[[attachments]]`.

use indicatrix_cut_core::native::{AttachmentBlob, DesignMetadata};

/// The `[meta]` table and attachments the open design came with.
///
/// Empty for a design that never had either (a new design, a bare `.asc`, an older
/// paired file). A Save starts from these, overlays what the design's library row says
/// (see `native_io::design_meta`), and writes the result.
#[derive(Debug, Clone, Default)]
pub(in crate::gui) struct DesignFileExtras {
    /// The design file's `[meta]` table as loaded, or as last written by a Save.
    pub(in crate::gui::editor) metadata: DesignMetadata,
    /// The design file's attachments as loaded, in file order.
    pub(in crate::gui::editor) attachments: Vec<AttachmentBlob>,
}

impl DesignFileExtras {
    /// The extras a loaded design file (or catalogue attachment) carried.
    pub(in crate::gui::editor) const fn new(
        metadata: DesignMetadata,
        attachments: Vec<AttachmentBlob>,
    ) -> Self {
        Self {
            metadata,
            attachments,
        }
    }
}
