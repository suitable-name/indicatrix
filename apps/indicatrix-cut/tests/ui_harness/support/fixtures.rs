//! Library fixtures: real `.asc` designs stored in the profile's design library before a
//! scenario's app launches, so list, search, viewer and planner scenarios have something to show.
//!
//! The designs are stored by [`harness_support::seed_library_asc`], which runs the same steps as
//! the app's own import (parse, measure the proportions, `save_design` under the local source),
//! so the rows are the ones an import would have made.

use super::profile::Profile;
use indicatrix_cut::harness_support;

/// One design to put in the harness library before launch.
pub struct FixtureDesign {
    /// The title the library list shows (the `.asc` file's first header line).
    pub title: &'static str,
    /// The file name the design is imported under (`local://<file_name>` is its key).
    pub file_name: &'static str,
    /// The `.asc` text.
    pub asc: String,
}

macro_rules! fixture {
    ($title:literal, $name:literal) => {
        FixtureDesign {
            title: $title,
            file_name: concat!($name, ".asc"),
            asc: include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../crates/indicatrix/tests/fixtures/asc/",
                $name,
                ".asc"
            ))
            .to_owned(),
        }
    };
}

/// The fixture set: the four `.asc` files in `crates/indicatrix/tests/fixtures/asc/`
/// (trichecker12, shah, texasstar, forfun).
///
/// The titles are the first `H` header of each file with the leading `PC` catalogue number
/// stripped by the parser; if a title differs in the library list, the file header is the truth.
#[must_use]
pub fn sample_designs() -> Vec<FixtureDesign> {
    vec![
        fixture!("Round Trichecker-12", "trichecker12"),
        fixture!("Shah (Replica)", "shah"),
        fixture!("Large Texas Star", "texasstar"),
        fixture!("For Fun", "forfun"),
    ]
}

/// Writes `designs` into the profile's library database (the file
/// `harness_support::database_path()` resolves to) through the vault, as the app's own import
/// would store them. The app must not be running.
///
/// # Errors
///
/// The database path lies outside the profile, or the first design that fails to parse or save.
pub fn seed_library(profile: &Profile, designs: &[FixtureDesign]) -> Result<(), String> {
    let database = harness_support::database_path();
    if !profile.contains(&database) {
        return Err(format!(
            "refusing to seed a library outside the profile: {}",
            database.display()
        ));
    }
    let files: Vec<(&str, &[u8])> = designs
        .iter()
        .map(|d| (d.file_name, d.asc.as_bytes()))
        .collect();
    harness_support::seed_library_asc(&files).map(|_| ())
}
