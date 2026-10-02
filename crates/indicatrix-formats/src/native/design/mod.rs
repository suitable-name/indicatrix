//! The self-contained `.indicatrix` design file -- see [`schema`] for the format, its
//! tables and an example.
//!
//! [`parse`]/[`to_string`] are the codec, [`detect_kind`]/[`detect_kind_of_path`] route
//! a file to the right loader (this one, or the older paired overlay sidecar in the
//! parent module), and [`is_design_path`]/[`design_path_for`] with the `DESIGN_*`
//! constants are the naming and media-type rules the apps share. Turning a
//! [`DesignFile`] into an editor design is `indicatrix-cut-core`'s `native` module.
//!
//! The plain-data types the vault and apps share are [`DesignMetadata`] (the `[meta]`
//! table) and [`AttachmentBlob`] (one `[[attachments]]` file with its bytes); the
//! stored-versus-recomputed field table is in [`schema`].

mod attachment;
mod base64;
mod codec;
mod meta;
mod path;
mod schema;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_meta;

pub use attachment::{
    AttachmentBlob, AttachmentProblem, AttachmentRole, AttachmentTable, AttachmentsError,
    MAX_ATTACHMENT_BYTES, MAX_ATTACHMENT_MIME_BYTES, MAX_ATTACHMENT_NAME_BYTES, MAX_ATTACHMENTS,
    attachment_blobs, attachment_tables, mime_type_for_name, validate_attachments,
};
pub use codec::{
    DesignFileError, FileKind, check_header, detect_kind, parse, to_string, validate,
    validate_extras,
};
pub use meta::{
    DesignMetadata, MAX_META_TAG_BYTES, MAX_META_TAGS, MAX_META_TEXT_BYTES, is_iso8601_utc, is_uuid,
};
pub use path::{
    DESIGN_ACCEPT, DESIGN_EXTENSION, DESIGN_EXTENSION_DOTTED, DESIGN_FILE_DESCRIPTION,
    DESIGN_MIME_TYPE, design_path_for, design_path_for_sibling, detect_kind_of_path,
    is_design_path,
};
pub use schema::{DESIGN_FORMAT, DESIGN_VERSION, DesignFile, MAX_DESIGN_TIERS, ScheduleTable};
