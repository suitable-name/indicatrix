//! Tolerant CSV reading and resampling onto the fixed grid.

use super::{GRID_LEN, SpectralError, grid_wavelength_nm};

/// Values below this are rejected; values in `[-NEGATIVE_LIMIT, 0)` are clamped to 0.
pub(super) const NEGATIVE_LIMIT: f64 = 1e-3;

/// Parse the first `columns` numeric fields of every data row.
///
/// Delimiters: comma, semicolon, tab, whitespace. Blank lines and lines starting with `#` are
/// skipped. A first non-numeric line (before any data row) is taken as a header; a bad line
/// after that is an error. Decimal commas are not supported.
pub(super) fn parse_table(text: &str, columns: usize) -> Result<Vec<Vec<f64>>, SpectralError> {
    let mut rows: Vec<Vec<f64>> = Vec::new();
    let mut header_skipped = false;
    for (line_index, raw) in text.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let fields: Vec<&str> = line
            .split(|c: char| c == ',' || c == ';' || c == '\t' || c.is_whitespace())
            .filter(|field| !field.is_empty())
            .collect();
        let parsed: Option<Vec<f64>> = if fields.len() >= columns {
            fields
                .iter()
                .take(columns)
                .map(|field| field.parse::<f64>().ok().filter(|v| v.is_finite()))
                .collect()
        } else {
            None
        };
        match parsed {
            Some(values) => rows.push(values),
            None if rows.is_empty() && !header_skipped => header_skipped = true,
            None => {
                return Err(SpectralError::Parse {
                    line: line_index + 1,
                    message: format!("expected {columns} finite numbers"),
                });
            }
        }
    }
    if rows.len() < 2 {
        return Err(SpectralError::TooFewRows { found: rows.len() });
    }
    Ok(rows)
}

/// Resample `values(wavelengths)` linearly onto the grid.
///
/// Wavelengths must increase strictly; values below -1e-3 are rejected, tiny negatives are
/// clamped to 0. Grid points outside the source range get 0.
pub(super) fn resample_to_grid(
    wavelengths: &[f64],
    values: &[f64],
) -> Result<[f64; GRID_LEN], SpectralError> {
    if wavelengths.len() != values.len() || wavelengths.len() < 2 {
        return Err(SpectralError::TooFewRows {
            found: wavelengths.len().min(values.len()),
        });
    }
    for (index, pair) in wavelengths.windows(2).enumerate() {
        if pair[1].partial_cmp(&pair[0]) != Some(core::cmp::Ordering::Greater) {
            return Err(SpectralError::NotMonotone { index: index + 1 });
        }
    }
    let mut clean = Vec::with_capacity(values.len());
    for (index, &value) in values.iter().enumerate() {
        if !value.is_finite() {
            return Err(SpectralError::InvalidInput(format!(
                "non-finite value in row {index}"
            )));
        }
        if value < -NEGATIVE_LIMIT {
            return Err(SpectralError::NegativeValue { index, value });
        }
        clean.push(value.max(0.0));
    }
    let first = wavelengths[0];
    let last = wavelengths[wavelengths.len() - 1];
    if last < grid_wavelength_nm(0) || first > grid_wavelength_nm(GRID_LEN - 1) {
        return Err(SpectralError::NoOverlap);
    }
    let mut out = [0.0; GRID_LEN];
    for (i, slot) in out.iter_mut().enumerate() {
        let lambda = grid_wavelength_nm(i);
        if lambda < first || lambda > last {
            continue;
        }
        let upper = wavelengths.partition_point(|&w| w <= lambda);
        if upper >= wavelengths.len() {
            *slot = clean[clean.len() - 1];
        } else {
            let lo = upper - 1;
            let t = (lambda - wavelengths[lo]) / (wavelengths[upper] - wavelengths[lo]);
            *slot = (clean[upper] - clean[lo]).mul_add(t, clean[lo]);
        }
    }
    Ok(out)
}

/// Read a two-column `wavelength, value` table onto the grid.
pub(super) fn read_curve(text: &str) -> Result<[f64; GRID_LEN], SpectralError> {
    let rows = parse_table(text, 2)?;
    let wavelengths: Vec<f64> = rows.iter().map(|row| row[0]).collect();
    let values: Vec<f64> = rows.iter().map(|row| row[1]).collect();
    resample_to_grid(&wavelengths, &values)
}
