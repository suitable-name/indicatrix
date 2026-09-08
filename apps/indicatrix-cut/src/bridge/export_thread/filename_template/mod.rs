//! Configurable export filename templates: `{design}`, `{material}`, `{spp}`, and
//! friends, resolved to a collision-free, filesystem-safe path inside the user's
//! configured export directory.
//!
//! [`TemplateContext`] is assembled once, in `gui::render_export` -- the one place with
//! the selected diagram, resolved material, and export request all in scope -- and
//! handed down fully resolved.
//!
//! # Threat model: `{design}`/`{designer}` come from a scraped catalogue
//!
//! `{design}`/`{designer}` come straight from `indicatrix_vault`'s SQLite-backed
//! catalogue, built by scraping third-party web pages -- nothing before this module has
//! validated that a design title can't contain a `/`, a trailing dot, a reserved
//! Windows device name, or literal `..`. [`sanitize_filename`] closes that risk for the
//! whole rendered filename in one pass, not per-variable, so no substitution can slip
//! past it.
//!
//! # Unknown/malformed variables
//!
//! `{spp}` misspelled as `{sp}`, or a stray `{` with no matching `}`: this module never
//! rejects a template outright. An unrecognised `{name}` is emitted literally, braces
//! included, so a typo stays visible instead of silently vanishing -- see [`render`].

use std::{
    path::{Path, PathBuf},
    time::SystemTime,
};

/// The default filename template. `{timestamp}` is deliberately last: it's the one
/// variable this module can still guarantee unique on its own, so keeping it at the end
/// means the uniqueness guarantee survives even if a user trims the template down. See
/// [`unique_path`] for the collision handling that covers a template that drops
/// `{timestamp}` entirely.
pub const DEFAULT_TEMPLATE: &str = "gem_export_{material}_{width}x{height}_{spp}spp_{timestamp}";

/// Every value a filename template may reference, gathered once per export (or once
/// per preset in a fan-out export) by the one caller with all of it in scope:
/// `gui::render_export`.
///
/// Every `String` field here is used RAW (not pre-sanitised) -- [`resolve_export_path`]
/// sanitises the fully-rendered filename in one pass, so callers don't need to think
/// about escaping at all.
#[derive(Debug, Clone)]
pub struct TemplateContext {
    /// The selected design's title (`DiagramDetailData::title` / `{design}`). Empty
    /// when no catalogue design is loaded.
    pub design: String,
    /// `DiagramDetailData::designer` (`{designer}`). Empty when not recorded.
    pub designer: String,
    /// `DiagramDetailData::shape` (`{shape}`).
    pub shape: String,
    /// The resolved material's display name (`{material}`) -- e.g. "Diamond" or a
    /// custom material's user-chosen name.
    pub material: String,
    /// `DiagramDetailData::ri` (`{ri}`), already formatted as this app displays it
    /// elsewhere -- not re-formatted here.
    pub ri: String,
    pub width: u32,
    pub height: u32,
    pub spp: u32,
    pub bounces: u32,
    /// The export's colour-space label (`{colorspace}`) -- e.g. "sRGB", "Display P3".
    pub colorspace: String,
    /// The lighting preset's name being rendered THIS file (`{preset}`) -- empty for
    /// the base current-view render in a fan-out export, the preset's own name for
    /// each additional one.
    pub preset: String,
    /// The lighting rig label actually in effect for this render (`{lighting}`).
    pub lighting: String,
    /// Camera yaw, in DEGREES (`{yaw}`) -- converted from `SceneSnapshot::yaw`'s
    /// radians at the call site: a filename is a human-facing surface, and nobody
    /// reads radians off one.
    pub yaw_deg: f32,
    pub pitch_deg: f32,
    pub distance: f32,
    pub exposure: f32,
}

/// Resolves one `{name}` template variable against `ctx`/`now`, or `None` for a name
/// this module doesn't recognise -- see [`render`] for what the caller does with `None`.
fn substitute(name: &str, ctx: &TemplateContext, now: SystemTime) -> Option<String> {
    let (secs, (year, month, day, hour, minute, second)) = {
        let secs = now
            .duration_since(SystemTime::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs() as i64);
        (secs, civil_from_unix_seconds(secs))
    };
    Some(match name {
        "design" => ctx.design.clone(),
        "designer" => ctx.designer.clone(),
        "shape" => ctx.shape.clone(),
        "material" => ctx.material.clone(),
        "ri" => ctx.ri.clone(),
        "width" => ctx.width.to_string(),
        "height" => ctx.height.to_string(),
        "spp" => ctx.spp.to_string(),
        "bounces" => ctx.bounces.to_string(),
        "colorspace" => ctx.colorspace.clone(),
        "preset" => ctx.preset.clone(),
        "lighting" => ctx.lighting.clone(),
        // One decimal place: enough to distinguish two poses without `f32`'s full
        // precision noise (`48.300004`).
        "yaw" => format!("{:.1}", ctx.yaw_deg),
        "pitch" => format!("{:.1}", ctx.pitch_deg),
        "distance" => format!("{:.2}", ctx.distance),
        "exposure" => format!("{:.2}", ctx.exposure),
        "date" => format!("{year:04}-{month:02}-{day:02}"),
        // Dashes, not colons: `:` is one of the characters `sanitize_filename` would
        // otherwise strip back out on Windows.
        "time" => format!("{hour:02}-{minute:02}-{second:02}"),
        "timestamp" => secs.to_string(),
        _ => return None,
    })
}

/// Substitutes every `{variable}` in `template` against `ctx`/`now`, leaving anything
/// unrecognised -- a misspelled variable, or a stray `{` with no matching `}` --
/// exactly as written, so a typo stays visible rather than vanishing.
///
/// `now` is threaded in as a parameter rather than calling `SystemTime::now()` itself
/// so `{date}`/`{time}`/`{timestamp}` are unit-testable against a known instant.
#[must_use]
pub fn render(template: &str, ctx: &TemplateContext, now: SystemTime) -> String {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let after_open = &rest[open + 1..];
        let Some(close) = after_open.find('}') else {
            // Unterminated `{`: emit it and everything after literally.
            out.push('{');
            out.push_str(after_open);
            rest = "";
            continue;
        };
        let name = &after_open[..close];
        if let Some(value) = substitute(name, ctx, now) {
            out.push_str(&value);
        } else {
            out.push('{');
            out.push_str(name);
            out.push('}');
        }
        rest = &after_open[close + 1..];
    }
    out.push_str(rest);
    out
}

/// Characters forbidden in a Windows filename, plus every ASCII control character --
/// see <https://learn.microsoft.com/en-us/windows/win32/fileio/naming-a-file>. Replaced
/// with `_` rather than stripped outright: dropping characters can collapse two
/// distinct titles (`"A/B"` and `"AB"`) into the same filename.
const fn is_forbidden_filename_char(c: char) -> bool {
    matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*') || (c as u32) < 0x20
}

/// Windows' reserved device names -- reserved with or without an extension
/// (`CON.png` is just as reserved as `CON`), case-insensitively. Checked against the
/// filename STEM (before `.png` is appended).
const RESERVED_WINDOWS_NAMES: &[&str] = &[
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

/// Sanitises a fully-rendered filename (stem, no extension yet) into something safe to
/// create on Windows -- runs on the whole rendered string in one pass rather than
/// per-variable.
///
/// Also makes a `..`-style directory-traversal attempt structurally impossible: every
/// path separator (`/` and `\`) is replaced with `_`, so the result can never contain
/// more than one path component. [`resolve_export_path`] still independently asserts
/// the final joined path stays inside the export directory.
fn sanitize_filename(raw: &str) -> String {
    let mut cleaned: String = raw
        .chars()
        .map(|c| {
            if is_forbidden_filename_char(c) {
                '_'
            } else {
                c
            }
        })
        .collect();

    // Windows silently strips/rejects trailing dots and spaces -- trim from the end
    // only, so an internal ". " sequence (e.g. "R. Strauss") survives untouched.
    while matches!(cleaned.chars().last(), Some('.' | ' ')) {
        cleaned.pop();
    }

    if cleaned.is_empty() {
        // Every substitution rendered empty and/or was forbidden -- fall back to a
        // name that is at least valid.
        cleaned = "export".to_string();
    }

    let stem_before_extension = cleaned.split('.').next().unwrap_or(&cleaned);
    if RESERVED_WINDOWS_NAMES
        .iter()
        .any(|reserved| reserved.eq_ignore_ascii_case(stem_before_extension))
    {
        cleaned = format!("_{cleaned}");
    }

    cleaned
}

/// Appends `.png` unless `name` already ends with it (case-insensitively) -- e.g. a
/// custom template ending in the literal text `.png`, or a `{design}` that happened to
/// already contain it, isn't double-suffixed into `Foo.png.png`.
fn ensure_png_extension(name: &str) -> String {
    if name.len() >= 4 && name[name.len() - 4..].eq_ignore_ascii_case(".png") {
        name.to_string()
    } else {
        format!("{name}.png")
    }
}

/// Finds a filename inside `dir` that doesn't already exist, appending ` (2)`, ` (3)`,
/// etc. before the extension like a browser's "already downloaded" convention, rather
/// than overwriting: a template like `{design}_{material}` produces the identical name
/// for every export of the same design in the same material.
///
/// `filename` must already end in `.png` (case-insensitive) -- [`resolve_export_path`]
/// guarantees this via [`ensure_png_extension`] before calling here.
fn unique_path(dir: &Path, filename: &str) -> PathBuf {
    let split_at = filename.len() - 4; // ".png" (or "*.PNG"/mixed case), 4 bytes, ASCII
    let (stem, extension) = filename.split_at(split_at);

    let mut candidate = dir.join(filename);
    let mut suffix = 2u32;
    while candidate.exists() {
        candidate = dir.join(format!("{stem} ({suffix}){extension}"));
        suffix += 1;
    }
    candidate
}

/// Renders `template` against `ctx`, sanitises it, appends `.png` if missing, and
/// resolves a collision-free path inside `export_dir`.
///
/// # The directory-escape assertion
///
/// [`sanitize_filename`] already makes an escape structurally impossible, but this
/// asserts it anyway (`dir.join(name)`'s parent must be exactly `export_dir`, `name`
/// must not be `.`/`..`) as cheap defense-in-depth against a future edit to
/// `sanitize_filename` reopening the escape. A real `assert!`, not `debug_assert!`,
/// since release builds are exactly where a scraped-catalogue title is most likely to
/// be weird.
///
/// # Panics
///
/// Panics if the resolved path would not live directly inside `export_dir`. Should be
/// unreachable given [`sanitize_filename`]'s guarantee.
#[must_use]
pub fn resolve_export_path(export_dir: &Path, template: &str, ctx: &TemplateContext) -> PathBuf {
    let rendered = render(template, ctx, SystemTime::now());
    let sanitized = sanitize_filename(&rendered);
    let named = ensure_png_extension(&sanitized);

    assert!(
        !named.contains('/') && !named.contains('\\'),
        "sanitize_filename must strip every path separator; got {named:?}"
    );
    assert!(
        named != "." && named != "..",
        "sanitize_filename must never produce a bare '.' or '..' component; got {named:?}"
    );

    let resolved = unique_path(export_dir, &named);
    assert_eq!(
        resolved.parent(),
        Some(export_dir),
        "resolved export path {} must live directly inside the export directory {}",
        resolved.display(),
        export_dir.display(),
    );
    resolved
}

/// Converts a Unix timestamp (whole seconds since the epoch, UTC) into
/// `(year, month, day, hour, minute, second)` without pulling in a date/time crate
/// just for `{date}`/`{time}`. Proleptic Gregorian, based on Howard Hinnant's
/// public-domain `civil_from_days` algorithm
/// (<http://howardhinnant.github.io/date_algorithms.html>), pinned by this function's
/// tests against several known dates including the 1970 epoch and a Feb-29 leap day.
const fn civil_from_unix_seconds(secs: i64) -> (i64, u32, u32, u32, u32, u32) {
    let days = secs.div_euclid(86_400);
    let time_of_day = secs.rem_euclid(86_400);
    let hour = (time_of_day / 3600) as u32;
    let minute = ((time_of_day % 3600) / 60) as u32;
    let second = (time_of_day % 60) as u32;

    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 }.div_euclid(146_097);
    let day_of_era = (z - era * 146_097) as u64; // [0, 146096]
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365; // [0, 399]
    let year = year_of_era as i64 + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100); // [0, 365]
    let mp = (5 * day_of_year + 2) / 153; // [0, 11]
    let day = (day_of_year - (153 * mp + 2) / 5 + 1) as u32; // [1, 31]
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32; // [1, 12]
    let year = if month <= 2 { year + 1 } else { year };

    (year, month, day, hour, minute, second)
}

#[cfg(test)]
mod tests;
