//! What a `.indicatrix` design file carries besides the design itself, kept for the
//! open design so the next Save writes it back unchanged: the `[meta]` table (with its
//! unknown keys) and the byte-exact `[[attachments]]`.

use super::design_identity::{fresh_design_uuid, resolve_design_uuid, resolve_file_design_uuid};
use indicatrix_cut_core::native::{AttachmentBlob, DesignMetadata};
use std::path::Path;

/// The `[meta]` table and attachments the open design came with.
///
/// Empty for a design that never had either (a new design, a bare `.asc`, an older
/// paired file) apart from the design's UUID, which every open design has (see
/// [`Self::with_design_uuid_assigned`]). A Save starts from these, overlays what the
/// design's library row says (see `native_io::design_meta`), and writes the result.
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

    /// These extras with the design's UUID (`[meta].id`) settled: the one the file
    /// already has, else -- for a design loaded from the local catalogue, whose entry
    /// `url` is `catalogue_url` -- the entry's deterministic UUID, else a fresh random
    /// one. See [`resolve_design_uuid`] for the rules.
    ///
    /// Every place that puts a design into the editor calls this, so the UUID exists
    /// from the moment the design opens, and the next Save or autosave writes it to the
    /// file.
    pub(in crate::gui::editor) fn with_design_uuid_assigned(
        mut self,
        catalogue_url: Option<&str>,
    ) -> Self {
        self.metadata.id = resolve_design_uuid(&self.metadata.id, catalogue_url, fresh_design_uuid);
        self
    }

    /// These extras with the design's UUID settled for a design opened from the file at
    /// `path`: the id the file already has, else the deterministic UUID of the file's
    /// location (see [`resolve_file_design_uuid`]), so a file that has no id yet gets the
    /// same one every time it is opened until a Save writes it in.
    pub(in crate::gui::editor) fn with_design_uuid_assigned_for_file(
        mut self,
        path: &Path,
    ) -> Self {
        self.metadata.id = resolve_file_design_uuid(&self.metadata.id, path, fresh_design_uuid);
        self
    }
}
