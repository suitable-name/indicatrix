//! Where the census gets its designs: the built-in templates and fixtures, design files and the
//! designs of a library (follow-ups plan 2026-10-19, sections 1.7 and 4.2).
//!
//! The library is read only through the vault's own read-only API
//! ([`Database::open_read_only`], `matching_entry_ids`, `get_diagram_full`). A record becomes a
//! design the way the desktop's Load Selected does it: the attached `.isdd` first, else the
//! attached `.asc` (or a converted `.gem` or `.gcs`) read with the record's length-to-width
//! ratio. The angle-table placeholder is never used: a record with no design file is one row
//! that says so.

use crate::{
    args::CensusArgs,
    load::{Loaded, from_bytes, load_path},
    materials::Catalogue,
    outcome::CliError,
};
use indicatrix_cut_core::{
    ConstraintTier, Design, MaterialSelection, PreformSpec, ScheduleMeta,
    native::{DesignExtras, design_to_string},
    templates::TEMPLATES,
};
use indicatrix_editor::{loading::design_from_asc_text, templates::gallery_design};
use indicatrix_vault::{
    db::sqlite::{Database, DisplayFilters},
    local::{
        design_attachment_position, design_file_to_asc_text, native_design_attachment_position,
    },
    model::{entry::FullDiagramRecord, filter::RangeFilter, frosting::StoredTierFrosting},
};
use std::path::{Path, PathBuf};

/// A built-in design that is not a template.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Fixture {
    /// `Design::concave_fixture`.
    Concave,
    /// The standard brilliant as a 1.72 stone, the Retarget engine's own test stone.
    Brilliant,
}

/// One design to measure, before it is opened.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum SourceRef {
    /// A gallery template, 1-based.
    Template(i32),
    /// A built-in fixture.
    Fixture(Fixture),
    /// A design file.
    File(PathBuf),
    /// A library entry id.
    Library(i64),
}

/// A design that has been opened, or why it could not be.
pub(super) struct Subject {
    /// `template`, `fixture`, `file` or `library`.
    pub source: &'static str,
    /// Template number, fixture name, file name or entry id.
    pub key: String,
    /// What to call it.
    pub title: String,
    /// The design, or the reason there is none (the row's `capture` text).
    pub design: Result<Design, String>,
    /// The opened file, which keeps what a `.isdd` holds besides the design.
    pub loaded: Option<Loaded>,
    /// The most decimals the schedule's masts are written with (`crate::load::mast_decimals`);
    /// `None` for a template, a fixture or a `.isdd` file.
    pub mast_decimals: Option<usize>,
}

/// The library at `path`, opened read-only.
///
/// # Errors
///
/// [`CliError::io`] when it cannot be opened.
pub(super) fn open_library(path: &Path) -> Result<Database, CliError> {
    Database::open_read_only(&path.to_string_lossy()).map_err(|error| {
        CliError::io(format!(
            "cannot open the design library {}: {error:#}",
            path.display()
        ))
    })
}

/// The designs `args` names, in census order: templates, fixtures, files, library.
///
/// # Errors
///
/// [`CliError::io`] when the library cannot list its entries.
pub(super) fn plan_sources(
    args: &CensusArgs,
    library: Option<&Database>,
) -> Result<Vec<SourceRef>, CliError> {
    let mut sources = Vec::new();
    if args.builtins.templates {
        for number in 1..=TEMPLATES.len() {
            if let Ok(number) = i32::try_from(number) {
                sources.push(SourceRef::Template(number));
            }
        }
    }
    if args.builtins.fixtures {
        sources.push(SourceRef::Fixture(Fixture::Concave));
        sources.push(SourceRef::Fixture(Fixture::Brilliant));
    }
    sources.extend(args.files.iter().cloned().map(SourceRef::File));
    if let Some(database) = library {
        let mut ids = if args.all {
            database
                .matching_entry_ids(
                    "",
                    "All",
                    "All",
                    &RangeFilter::default(),
                    DisplayFilters::default(),
                )
                .map_err(|error| {
                    CliError::io(format!("cannot list the design library: {error:#}"))
                })?
        } else {
            args.ids.clone()
        };
        ids.sort_unstable();
        ids.dedup();
        let wanted = ids.into_iter().skip(args.offset);
        match args.limit {
            Some(limit) => sources.extend(wanted.take(limit).map(SourceRef::Library)),
            None => sources.extend(wanted.map(SourceRef::Library)),
        }
    }
    Ok(sources)
}

/// Opens one design; a failure is the subject's `design` text, never an error.
pub(super) fn open_subject(source: &SourceRef, library: Option<&Database>) -> Subject {
    match source {
        SourceRef::Template(number) => template_subject(*number),
        SourceRef::Fixture(fixture) => fixture_subject(*fixture),
        SourceRef::File(path) => file_subject(path),
        SourceRef::Library(id) => library_subject(*id, library),
    }
}

fn template_subject(number: i32) -> Subject {
    let name = usize::try_from(number - 1)
        .ok()
        .and_then(|index| TEMPLATES.get(index))
        .map_or("template", |spec| spec.name);
    Subject {
        source: "template",
        key: number.to_string(),
        title: name.to_string(),
        design: Ok(gallery_design(number)),
        loaded: None,
        mast_decimals: None,
    }
}

/// The standard brilliant as a 1.72 stone (the construction of the CLI test fixture).
fn brilliant_at_172() -> Design {
    let mut design = Design::new(
        PreformSpec::block(2.0, 1.0, 4.0),
        ScheduleMeta::standard_round_brilliant(),
        ConstraintTier::standard_round_brilliant(),
    );
    design.material = MaterialSelection {
        name: None,
        specific_gravity_override: None,
        refractive_index_override: Some(1.72),
        body_color_override: None,
        body_color_bands_override: None,
        absorption_path_scale_override: None,
    };
    design.ensure_tier_ids();
    design
}

fn fixture_subject(fixture: Fixture) -> Subject {
    let (key, title, design) = match fixture {
        Fixture::Concave => (
            "concave_fixture",
            "Concave Fixture",
            Design::concave_fixture(),
        ),
        Fixture::Brilliant => ("brilliant_172", "Brilliant at 1.72", brilliant_at_172()),
    };
    Subject {
        source: "fixture",
        key: key.to_string(),
        title: title.to_string(),
        design: Ok(design),
        loaded: None,
        mast_decimals: None,
    }
}

fn file_subject(path: &Path) -> Subject {
    let key = path.file_name().map_or_else(
        || path.display().to_string(),
        |name| name.to_string_lossy().into_owned(),
    );
    match load_path(path) {
        Ok(loaded) => Subject {
            source: "file",
            title: loaded.name.clone(),
            key,
            design: Ok(loaded.design.clone()),
            mast_decimals: loaded.mast_decimals,
            loaded: Some(loaded),
        },
        Err(error) => Subject {
            source: "file",
            title: key.clone(),
            key,
            design: Err(error.message),
            loaded: None,
            mast_decimals: None,
        },
    }
}

fn library_subject(id: i64, library: Option<&Database>) -> Subject {
    let failed = |title: String, why: String| Subject {
        source: "library",
        key: id.to_string(),
        title,
        design: Err(why),
        loaded: None,
        mast_decimals: None,
    };
    let Some(database) = library else {
        return failed(String::new(), "no design library".to_string());
    };
    let full = match database.get_diagram_full(id) {
        Ok(Some(full)) => full,
        Ok(None) => return failed(String::new(), "no such library entry".to_string()),
        Err(error) => return failed(String::new(), format!("cannot read the entry: {error:#}")),
    };
    match design_from_record(&full) {
        Ok((design, loaded, mast_decimals)) => Subject {
            source: "library",
            key: id.to_string(),
            title: full.title,
            design: Ok(design),
            loaded,
            mast_decimals,
        },
        Err(why) => failed(full.title, why),
    }
}

/// The design of a library record, the opened file when it came from a `.isdd`, and the most
/// decimals its schedule writes a mast with when it came from a schedule.
///
/// # Errors
///
/// `no design file` when the record attaches none, else the reader's own message.
pub(super) fn design_from_record(
    full: &FullDiagramRecord,
) -> Result<(Design, Option<Loaded>, Option<usize>), String> {
    let names = || full.attached_files.iter().map(|file| file.name.as_str());
    if let Some(position) = native_design_attachment_position(names()) {
        let file = &full.attached_files[position];
        let loaded = from_bytes(&file.name, &file.content).map_err(|error| error.message)?;
        return Ok((loaded.design.clone(), Some(loaded), None));
    }
    let (position, kind) =
        design_attachment_position(names()).ok_or_else(|| "no design file".to_string())?;
    let file = &full.attached_files[position];
    let text = design_file_to_asc_text(&file.name, kind, &file.content)
        .map_err(|error| format!("{}: {error}", file.name))?;
    let frosting = if full.frosting.is_empty() {
        text.frosting
    } else {
        StoredTierFrosting::to_gcs(&full.frosting)
    };
    let mut loaded = design_from_asc_text(
        &text.asc_file_name,
        &text.asc_text,
        full.lw_ratio.as_deref(),
    )
    .map_err(|error| format!("{}: {error}", file.name))?;
    loaded.design.apply_imported_frosting(&frosting);
    let decimals = crate::load::mast_decimals(&text.asc_text);
    Ok((loaded.design, None, decimals))
}

/// The text of a `.isdd` file holding `design`, around whatever the subject's file held.
///
/// # Errors
///
/// [`CliError::design`] when the design cannot be written.
pub(super) fn design_text(
    subject: &Subject,
    design: &Design,
    catalogue: &Catalogue,
) -> Result<String, CliError> {
    if let Some(loaded) = &subject.loaded {
        return loaded.design_text(design, catalogue);
    }
    let snapshot = design
        .material
        .name
        .as_deref()
        .and_then(|name| catalogue.library_snapshot(name));
    let extras = DesignExtras {
        custom_material: snapshot,
        history_entries: &[],
        metadata: None,
        attachments: &[],
    };
    design_to_string(design, None, &extras).map_err(|error| {
        CliError::design(format!("the design cannot be written as a file: {error}"))
    })
}
