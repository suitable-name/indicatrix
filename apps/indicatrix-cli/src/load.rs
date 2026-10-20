//! Opening a design file: `.isdd` (or a legacy `.indicatrix`), `.asc`, `.gem` and `.gcs`,
//! through the same shared entry points the desktop's Open uses.
//!
//! | File | Path |
//! |---|---|
//! | `.isdd`, `.indicatrix` | `indicatrix_cut_core::native::design_from_str` |
//! | `.asc` | `indicatrix_formats::asc::decode_asc_bytes`, then `indicatrix_editor::loading::design_from_asc_text` |
//! | `.gem`, `.gcs` | `indicatrix_editor::files::convert_foreign_design`, then the `.asc` path |
//!
//! A `.isdd` file carries more than the design: the printed proportions of the catalogue
//! row it came from, a custom-material snapshot, a history trail, the `[meta]` table and its
//! attachments. [`Loaded`] keeps all of it, and [`Loaded::design_text`] writes it back
//! unchanged around whatever design a command produced, so `solve --out` and `retarget --out`
//! never drop what the file held. Nothing is minted: no new id and no timestamp, which is what
//! keeps the output deterministic.

use crate::{materials::Catalogue, outcome::CliError};
use indicatrix::{geometry::stone_metrics::ExternalProportions, optics::materials::GemMaterial};
use indicatrix_cut_core::{
    Design,
    native::{
        AttachmentBlob, CustomMaterialSnapshot, DesignExtras, DesignMetadata, design_from_str,
        design_to_string, gem_material_from_custom_snapshot,
    },
};
use indicatrix_editor::{
    cut_sheet::SheetDetails,
    files::{InputFileKind, convert_foreign_design},
    loading::design_from_asc_text,
};
use indicatrix_formats::{asc::decode_asc_bytes, native::design::DESIGN_EXTENSION_DOTTED};
use std::path::Path;

/// What a `.isdd` file holds besides the design itself.
#[derive(Debug, Default)]
struct Kept {
    printed_proportions: Option<ExternalProportions>,
    custom_material: Option<CustomMaterialSnapshot>,
    history_entries: Vec<String>,
    metadata: DesignMetadata,
    attachments: Vec<AttachmentBlob>,
}

/// A design file that has been opened.
#[derive(Debug)]
pub struct Loaded {
    /// The design.
    pub design: Design,
    /// What to call it: its first header line, else the file's name without its extension.
    pub name: String,
    /// The file's own name, without its folder.
    pub file_name: String,
    /// The kind of file it was: `.isdd` (also for a legacy `.indicatrix`), `.asc`, `.gem` or
    /// `.gcs`.
    pub source: &'static str,
    /// What the reader noted: a draft flag, converter warnings.
    pub notes: Vec<String>,
    /// The most decimals a facet record of the schedule writes its mast with ([`mast_decimals`]);
    /// `None` for a `.isdd` file, which stores no schedule text.
    pub mast_decimals: Option<usize>,
    kept: Kept,
}

/// The most decimals any facet record (`a` line) of the `.asc` text `asc_text` writes its mast
/// with: 5 for `GemCad` 4.x, 8 for `GemCad` 5. A five-decimal mast is quantised to `1e-5`, which
/// opens a designed meet by a few millionths of the stone. `None` when there is no facet record.
#[must_use]
pub fn mast_decimals(asc_text: &str) -> Option<usize> {
    asc_text
        .lines()
        .filter_map(|line| {
            let mut tokens = line.split_whitespace();
            if tokens.next()? != "a" {
                return None;
            }
            let mast = tokens.nth(1)?;
            Some(mast.split_once('.').map_or(0, |(_, fraction)| {
                fraction.chars().take_while(char::is_ascii_digit).count()
            }))
        })
        .max()
}

/// The design's display name: its first non-blank header line, else `file_name` without its
/// extension.
fn display_name(design: &Design, file_name: &str) -> String {
    design
        .meta
        .headers
        .iter()
        .map(|header| header.trim())
        .find(|header| !header.is_empty())
        .map_or_else(
            || {
                Path::new(file_name).file_stem().map_or_else(
                    || file_name.to_string(),
                    |stem| stem.to_string_lossy().into_owned(),
                )
            },
            str::to_string,
        )
}

impl Loaded {
    fn new(
        design: Design,
        file_name: &str,
        source: &'static str,
        notes: Vec<String>,
        kept: Kept,
    ) -> Self {
        Self {
            name: display_name(&design, file_name),
            design,
            file_name: file_name.to_string(),
            source,
            notes,
            mast_decimals: None,
            kept,
        }
    }

    /// A design that did not come from a file, for the tests.
    #[cfg(test)]
    pub fn in_memory(design: Design, file_name: &str) -> Self {
        Self::new(
            design,
            file_name,
            DESIGN_EXTENSION_DOTTED,
            Vec::new(),
            Kept::default(),
        )
    }

    /// The custom material the file carried, as a `GemMaterial` under the design's own
    /// material name; `None` when the file carried none.
    #[must_use]
    pub fn file_material(&self) -> Option<GemMaterial> {
        let name = self.design.material.name.as_deref()?;
        let snapshot = self.kept.custom_material.as_ref()?;
        Some(gem_material_from_custom_snapshot(name, snapshot))
    }

    /// The header words of a printed cutting sheet for this file: the `.isdd` file's own
    /// title, designer, shape and notes where it has them, else the design's `.asc` header and
    /// footnote lines (the only source for any other kind of file). `date_text` is printed as
    /// given; the sheet never reads a clock.
    #[must_use]
    pub fn sheet_details(&self, date_text: &str) -> SheetDetails {
        SheetDetails::from_metadata(&self.design, &self.kept.metadata, date_text)
    }

    /// The catalogue of this file: the library at `db`, and the file's own custom material.
    ///
    /// # Errors
    ///
    /// [`CliError::io`] when the library cannot be opened.
    pub fn catalogue(&self, db: Option<&Path>) -> Result<Catalogue, CliError> {
        Catalogue::open(db, self.file_material())
    }

    /// The text of a `.isdd` file holding `design` and everything this file held.
    ///
    /// The custom-material snapshot written is the one of the library material `design` names
    /// (`catalogue`, built as the desktop's Save builds it: see [`crate::materials`]), else the
    /// file's own, and the file's own only while `design` still names the material the file
    /// named: after a retarget to another material it would describe the wrong one. A design on
    /// a built-in material carries no snapshot.
    ///
    /// # Errors
    ///
    /// [`CliError::design`] when the design cannot be written (a `[meta]` value or attachment
    /// the file format refuses).
    pub fn design_text(&self, design: &Design, catalogue: &Catalogue) -> Result<String, CliError> {
        let same_material = match (&design.material.name, &self.design.material.name) {
            (Some(now), Some(then)) => now.eq_ignore_ascii_case(then),
            _ => false,
        };
        let library_snapshot = design
            .material
            .name
            .as_deref()
            .and_then(|name| catalogue.library_snapshot(name));
        let extras = DesignExtras {
            custom_material: library_snapshot.or_else(|| {
                same_material
                    .then_some(self.kept.custom_material.as_ref())
                    .flatten()
            }),
            history_entries: &self.kept.history_entries,
            metadata: Some(&self.kept.metadata),
            attachments: &self.kept.attachments,
        };
        design_to_string(design, self.kept.printed_proportions.as_ref(), &extras).map_err(|error| {
            CliError::design(format!(
                "the design cannot be written as a {DESIGN_EXTENSION_DOTTED} file: {error}"
            ))
        })
    }
}

/// Reads and opens the design file at `path`.
///
/// # Errors
///
/// [`CliError::io`] when the file cannot be read; [`CliError::design`] when it is not a design
/// this tool opens or does not parse.
pub fn load_path(path: &Path) -> Result<Loaded, CliError> {
    let bytes = std::fs::read(path)
        .map_err(|error| CliError::io(format!("cannot read {}: {error}", path.display())))?;
    let file_name = path.file_name().map_or_else(
        || path.display().to_string(),
        |name| name.to_string_lossy().into_owned(),
    );
    from_bytes(&file_name, &bytes)
}

/// Opens a design from `bytes` under the name `file_name` (which decides what kind of file it
/// is, except that a `.toml` is judged by its header).
///
/// # Errors
///
/// [`CliError::design`] when the kind is not a design this tool opens or the content does not
/// parse.
pub fn from_bytes(file_name: &str, bytes: &[u8]) -> Result<Loaded, CliError> {
    let Some(kind) = InputFileKind::classify(file_name, bytes) else {
        return Err(CliError::design(format!(
            "{file_name}: not a design file this tool opens (expected {DESIGN_EXTENSION_DOTTED}, .asc, .gem or .gcs)"
        )));
    };
    match kind {
        InputFileKind::Asc => from_asc(file_name, bytes),
        InputFileKind::Design => from_native(file_name, bytes),
        InputFileKind::Gem | InputFileKind::Gcs => from_foreign(file_name, kind, bytes),
        InputFileKind::Hdr => Err(CliError::design(format!(
            "{file_name}: a Radiance .hdr file is an environment map, not a design"
        ))),
    }
}

fn from_asc(file_name: &str, bytes: &[u8]) -> Result<Loaded, CliError> {
    let text = decode_asc_bytes(bytes);
    let loaded = design_from_asc_text(file_name, &text, None).map_err(|error| {
        CliError::design(format!(
            "{file_name}: not a readable .asc cutting schedule: {error}"
        ))
    })?;
    let mut opened = Loaded::new(
        loaded.design,
        file_name,
        ".asc",
        Vec::new(),
        Kept::default(),
    );
    opened.mast_decimals = mast_decimals(&text);
    Ok(opened)
}

fn from_foreign(file_name: &str, kind: InputFileKind, bytes: &[u8]) -> Result<Loaded, CliError> {
    let converted = convert_foreign_design(file_name, kind, bytes)
        .map_err(|error| CliError::design(format!("{file_name}: {error}")))?;
    let mut loaded = design_from_asc_text(&converted.asc_file_name, &converted.asc_text, None)
        .map_err(|error| {
            CliError::design(format!(
                "{file_name}: the converted schedule does not parse: {error}"
            ))
        })?;
    loaded.design.apply_imported_frosting(&converted.frosting);
    let notes = converted
        .warnings
        .iter()
        .map(|warning| format!("converter: {warning}"))
        .collect();
    let source = if kind == InputFileKind::Gem {
        ".gem"
    } else {
        ".gcs"
    };
    let mut opened = Loaded::new(loaded.design, file_name, source, notes, Kept::default());
    opened.mast_decimals = mast_decimals(&converted.asc_text);
    Ok(opened)
}

fn from_native(file_name: &str, bytes: &[u8]) -> Result<Loaded, CliError> {
    let text = std::str::from_utf8(bytes).map_err(|_| {
        CliError::design(format!(
            "{file_name}: a {DESIGN_EXTENSION_DOTTED} file is UTF-8 text, and this one is not"
        ))
    })?;
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let loaded =
        design_from_str(text).map_err(|error| CliError::design(format!("{file_name}: {error}")))?;
    let mut notes = Vec::new();
    if loaded.draft {
        notes.push(
            "the file was saved as a draft because the design did not solve at the time"
                .to_string(),
        );
    }
    let kept = Kept {
        printed_proportions: loaded.printed_proportions,
        custom_material: loaded.restorable_custom_material,
        history_entries: loaded.history_entries,
        metadata: loaded.metadata,
        attachments: loaded.attachments,
    };
    Ok(Loaded::new(
        loaded.design,
        file_name,
        DESIGN_EXTENSION_DOTTED,
        notes,
        kept,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        outcome::{EXIT_DESIGN, EXIT_IO},
        testing,
    };
    use indicatrix_formats::native::design::LEGACY_DESIGN_EXTENSION_DOTTED;

    const TINY_ASC: &str =
        "GemCad 5.0\ng 96 0.0\ny 4 y\nI 1.54\na -41.000000 0.64991234 92 n 1 84\n";

    #[test]
    fn an_asc_file_opens_and_is_named_after_its_file() {
        let loaded = from_bytes("tiny.asc", TINY_ASC.as_bytes()).expect("opens");
        assert_eq!(loaded.design.tiers.len(), 1);
        assert_eq!(loaded.name, "tiny");
        assert_eq!(loaded.source, ".asc");
        assert_eq!(loaded.file_name, "tiny.asc");
        assert_eq!(loaded.notes.len(), 0);
    }

    #[test]
    fn a_header_line_names_the_design() {
        let mut design = testing::template();
        design.meta.headers = vec!["  ".to_string(), "  My Stone ".to_string()];
        assert_eq!(display_name(&design, "x.asc"), "My Stone");
        design.meta.headers.clear();
        assert_eq!(display_name(&design, "stones/x.asc"), "x");
    }

    #[test]
    fn a_bad_asc_names_the_file() {
        let error = from_bytes("junk.asc", b"this is not a schedule").expect_err("not a schedule");
        assert_eq!(error.code, EXIT_DESIGN);
        assert!(error.message.starts_with("junk.asc:"), "{}", error.message);
    }

    #[test]
    fn a_design_file_round_trips_its_tiers_and_material() {
        let design = testing::template();
        let first = Loaded::in_memory(design.clone(), "stone.isdd");
        let none = Catalogue::default();
        let text = first.design_text(&design, &none).expect("writes");
        let again = from_bytes("stone.isdd", text.as_bytes()).expect("opens");
        assert_eq!(again.source, DESIGN_EXTENSION_DOTTED);
        assert_eq!(testing::shape(&again.design), testing::shape(&design));
        assert_eq!(again.design.material.name, design.material.name);
        // Writing it again gives the same bytes: nothing is minted.
        let second = again
            .design_text(&again.design, &none)
            .expect("writes again");
        assert_eq!(text, second);
    }

    #[test]
    fn a_legacy_indicatrix_file_opens_like_an_isdd_file() {
        let design = testing::template();
        let text = Loaded::in_memory(design.clone(), "old.isdd")
            .design_text(&design, &Catalogue::default())
            .expect("writes");
        let legacy = from_bytes(
            &format!("old{LEGACY_DESIGN_EXTENSION_DOTTED}"),
            text.as_bytes(),
        )
        .expect("a legacy file opens");
        assert_eq!(legacy.source, DESIGN_EXTENSION_DOTTED);
        assert_eq!(testing::shape(&legacy.design), testing::shape(&design));
    }

    #[test]
    fn a_byte_order_mark_is_tolerated() {
        let design = testing::template();
        let text = Loaded::in_memory(design.clone(), "s.isdd")
            .design_text(&design, &Catalogue::default())
            .expect("writes");
        let mut bytes = vec![0xEF, 0xBB, 0xBF];
        bytes.extend_from_slice(text.as_bytes());
        let loaded = from_bytes("s.isdd", &bytes).expect("opens with a BOM");
        assert_eq!(testing::shape(&loaded.design), testing::shape(&design));
    }

    #[test]
    fn a_design_file_that_is_not_text_is_refused() {
        let error = from_bytes("bad.isdd", &[0xFF, 0xFE, 0x00, 0x01]).expect_err("not text");
        assert_eq!(error.code, EXIT_DESIGN);
    }

    #[test]
    fn files_the_tool_does_not_open_are_refused_by_kind() {
        let unknown = from_bytes("notes.txt", b"hello").expect_err("unknown kind");
        assert!(
            unknown.message.contains("not a design file"),
            "{}",
            unknown.message
        );
        let hdr = from_bytes("sky.hdr", b"#?RADIANCE").expect_err("environment map");
        assert!(hdr.message.contains("environment map"), "{}", hdr.message);
        // An older sidecar is no design file any more: a plain "not a design file" refusal.
        let sidecar = from_bytes("old.indicatrix.toml", b"[preform]\n").expect_err("sidecar");
        assert!(
            sidecar.message.contains("not a design file"),
            "{}",
            sidecar.message
        );
        for error in [unknown, hdr, sidecar] {
            assert_eq!(error.code, EXIT_DESIGN);
        }
    }

    #[test]
    fn a_missing_file_is_an_io_error() {
        let error = load_path(&testing::temp_path("absent.isdd")).expect_err("absent");
        assert_eq!(error.code, EXIT_IO);
        assert!(error.message.contains("cannot read"), "{}", error.message);
    }

    #[test]
    fn a_file_on_disk_opens_like_its_bytes() {
        let path = testing::temp_path("on-disk.asc");
        std::fs::write(&path, TINY_ASC).expect("write the fixture");
        let loaded = load_path(&path).expect("opens");
        assert_eq!(loaded.design.tiers.len(), 1);
        assert_eq!(loaded.file_name, "on-disk.asc");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn the_file_material_is_only_there_when_the_file_had_a_snapshot() {
        let loaded = Loaded::in_memory(testing::template(), "s.isdd");
        assert!(loaded.file_material().is_none());
        assert_eq!(
            loaded.catalogue(None).expect("no library").custom().len(),
            0
        );
    }
}
