//! The reference filter set of the camera calibration (plan 4.3, tier 2).
//!
//! The set is one small text file. Every line that is not empty and does not start with `#` is
//!
//! ```text
//! name, r, g, b, spectrum-file
//! ```
//!
//! where `r, g, b` are the camera values measured for the filter on the empty backlit rig
//! (linear, relative to the empty rig: a photo of the filter divided by the white frame) and
//! `spectrum-file` is a `wavelength_nm, transmission` table (fractions, not percent), given
//! relative to the folder of the set file. Commas, semicolons or tabs separate the fields. One
//! leading header line whose `r` field is not a number is skipped.

use indicatrix_cut_core::rough_plan::camera_spectral::{ReferenceFilter, SpectralError};
use std::path::{Path, PathBuf};

/// One line of the set.
#[derive(Debug, Clone, PartialEq)]
pub struct FilterEntry {
    /// The filter's name.
    pub name: String,
    /// The measured camera values.
    pub rgb: [f64; 3],
    /// The spectrum file, as written.
    pub spectrum: String,
}

/// Why a set could not be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FilterSetError {
    /// A line has fewer than five fields, or a number that is not one (1-based line).
    Line {
        /// The line number.
        line: usize,
        /// What is wrong.
        message: &'static str,
    },
    /// The set has no filter lines at all.
    Empty,
}

impl std::fmt::Display for FilterSetError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Line { line, message } => write!(f, "line {line}: {message}"),
            Self::Empty => write!(f, "the filter set has no filters"),
        }
    }
}

impl std::error::Error for FilterSetError {}

fn fields(line: &str) -> Vec<&str> {
    let delimiter = if line.contains('\t') {
        '\t'
    } else if line.contains(';') {
        ';'
    } else {
        ','
    };
    line.split(delimiter).map(str::trim).collect()
}

/// Parses the text of a filter-set file.
///
/// # Errors
///
/// [`FilterSetError`] for a malformed line or an empty set.
pub fn parse_filter_set(text: &str) -> Result<Vec<FilterEntry>, FilterSetError> {
    let mut entries = Vec::new();
    let mut header_allowed = true;
    for (index, raw) in text.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let parts = fields(line);
        let numbers: Vec<Option<f64>> = parts
            .iter()
            .skip(1)
            .take(3)
            .map(|p| p.parse::<f64>().ok().filter(|v| v.is_finite()))
            .collect();
        if header_allowed && parts.len() >= 2 && numbers.first().copied().flatten().is_none() {
            header_allowed = false;
            continue;
        }
        header_allowed = false;
        if parts.len() < 5 {
            return Err(FilterSetError::Line {
                line: index + 1,
                message: "expected name, r, g, b and a spectrum file",
            });
        }
        let (Some(r), Some(g), Some(b)) = (numbers[0], numbers[1], numbers[2]) else {
            return Err(FilterSetError::Line {
                line: index + 1,
                message: "r, g and b must be numbers",
            });
        };
        if parts[0].is_empty() || parts[4].is_empty() {
            return Err(FilterSetError::Line {
                line: index + 1,
                message: "the name and the spectrum file must not be empty",
            });
        }
        entries.push(FilterEntry {
            name: parts[0].to_owned(),
            rgb: [r, g, b],
            spectrum: parts[4].to_owned(),
        });
    }
    if entries.is_empty() {
        Err(FilterSetError::Empty)
    } else {
        Ok(entries)
    }
}

/// The path of an entry's spectrum file: absolute paths stay, others are relative to the folder
/// of the set file.
#[must_use]
pub fn spectrum_path(set_file: &Path, entry: &FilterEntry) -> PathBuf {
    let path = Path::new(&entry.spectrum);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        set_file
            .parent()
            .map_or_else(|| path.to_path_buf(), |dir| dir.join(path))
    }
}

/// Reads a filter-set file and the spectrum files it names.
///
/// # Errors
///
/// A sentence for the window: unreadable files, malformed lines, rejected spectra.
pub fn load_filter_set(set_file: &Path) -> Result<Vec<ReferenceFilter>, String> {
    let text = std::fs::read_to_string(set_file)
        .map_err(|e| format!("Could not read {}: {e}", set_file.display()))?;
    let entries = parse_filter_set(&text).map_err(|e| format!("{}: {e}", set_file.display()))?;
    let mut filters = Vec::with_capacity(entries.len());
    for entry in &entries {
        let path = spectrum_path(set_file, entry);
        let table = std::fs::read_to_string(&path)
            .map_err(|e| format!("Could not read {}: {e}", path.display()))?;
        let filter = ReferenceFilter::from_csv(&entry.name, &table, entry.rgb)
            .map_err(|e: SpectralError| format!("Filter {}: {e}", entry.name))?;
        filters.push(filter);
    }
    Ok(filters)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_lines_comments_and_a_header() {
        let text = "# my filters\nname, r, g, b, file\n\nred, 0.9, 0.1, 0.05, red.csv\nblue;0.1;0.2;0.8;sub/blue.csv\n";
        let entries = parse_filter_set(text).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].name, "red");
        assert_eq!(entries[0].rgb, [0.9, 0.1, 0.05]);
        assert_eq!(entries[1].spectrum, "sub/blue.csv");
        assert_eq!(entries[1].rgb, [0.1, 0.2, 0.8]);
    }

    #[test]
    fn tabs_work_and_a_file_without_header_is_fine() {
        let entries = parse_filter_set("g\t0.2\t0.9\t0.1\tg.csv").unwrap();
        assert_eq!(entries[0].name, "g");
    }

    #[test]
    fn bad_lines_are_named() {
        let e = parse_filter_set("a, 0.1, 0.2, 0.3, a.csv\nb, 0.1, x, 0.3, b.csv").unwrap_err();
        assert_eq!(
            e,
            FilterSetError::Line {
                line: 2,
                message: "r, g and b must be numbers"
            }
        );
        let e = parse_filter_set("a, 0.1, 0.2").unwrap_err();
        assert!(matches!(e, FilterSetError::Line { line: 1, .. }));
        assert_eq!(
            parse_filter_set("# only a comment\n\n"),
            Err(FilterSetError::Empty)
        );
        assert_eq!(
            parse_filter_set("a, 0.1, 0.2, 0.3, "),
            Err(FilterSetError::Line {
                line: 1,
                message: "the name and the spectrum file must not be empty"
            })
        );
    }

    #[test]
    fn spectrum_paths_follow_the_set_file() {
        let entry = FilterEntry {
            name: "a".into(),
            rgb: [0.0; 3],
            spectrum: "spectra/a.csv".into(),
        };
        let path = spectrum_path(Path::new("/data/set.csv"), &entry);
        assert_eq!(path, Path::new("/data").join("spectra/a.csv"));
        let absolute = std::env::temp_dir().join("abs.csv");
        let entry = FilterEntry {
            spectrum: absolute.to_string_lossy().into_owned(),
            ..entry
        };
        assert_eq!(spectrum_path(Path::new("/data/set.csv"), &entry), absolute);
    }

    #[test]
    fn a_missing_file_is_a_sentence() {
        let message = load_filter_set(Path::new("/definitely/not/here/set.csv")).unwrap_err();
        assert!(message.starts_with("Could not read"));
    }
}
