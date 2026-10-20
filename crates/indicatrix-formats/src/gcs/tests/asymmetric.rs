//! Index positions that are not mirrored about index 0 (a design with no mirror symmetry).
//!
//! The real `.gcs` corpus has none: every file is symmetric about index 0, so it cannot show
//! whether the tooth survives the writer and the reader in the direction it was cut. These
//! tests pin the round trip alone: what `to_gcs_string` writes, `parse_gcs` reads back at the
//! same teeth. They say nothing about how Gem Cut Studio itself reads such a file; that is the
//! owner's check (see the documentation of `to_gcs_string`), and the ignored test below writes
//! the files to check with.

use super::{index_set, octagon_schedule};
use crate::{
    asc::{AscSchedule, to_asc_string},
    gcs::{parse_gcs, to_gcs_string},
};

/// The octagon of the other tests with three crown and three pavilion teeth that have no
/// partner at the mirrored tooth.
fn not_mirrored() -> AscSchedule {
    let mut schedule = octagon_schedule();
    schedule.mirror = false;
    schedule.tiers[0].indices = vec![5.0, 20.0, 41.0];
    schedule.tiers[2].indices = vec![5.0, 20.0, 41.0];
    schedule
}

#[test]
fn positions_that_are_not_mirrored_about_zero_come_back_at_the_same_teeth() {
    let schedule = not_mirrored();
    let design = parse_gcs(&to_gcs_string(&schedule).expect("writable")).expect("parses");
    for tier in [0, 2] {
        let set: Vec<f64> = index_set(&design, tier)
            .iter()
            .map(|tooth| tooth.round())
            .collect();
        assert_eq!(set, [5.0, 20.0, 41.0], "tier {tier}");
    }
}

#[test]
fn a_negative_gear_reads_back_at_the_mirrored_teeth() {
    // A negative gear counts the other way (`GemCAD`'s chiral designs, like the Wittelsbach
    // replica with gear -96): the writer folds the sign into the tooth, and the file's own gear
    // is the plain tooth count, so the teeth read back are the mirrored ones.
    let mut schedule = not_mirrored();
    schedule.gear_teeth = -96;
    let design = parse_gcs(&to_gcs_string(&schedule).expect("writable")).expect("parses");
    assert_eq!(design.index.gear, 96);
    let set: Vec<f64> = index_set(&design, 0)
        .iter()
        .map(|tooth| tooth.round())
        .collect();
    assert_eq!(set, [55.0, 76.0, 91.0]);
}

/// Writes the pair of files for the check in Gem Cut Studio that the corpus cannot give:
/// `chiral-check.asc` (open it in `GemCAD`, or import it in Gem Cut Studio) and
/// `chiral-check.gcs` (what this writer makes of it). In `GemCAD` the crown facets at teeth
/// 5, 20 and 41 sit directly above the pavilion facets at the same teeth. Open the `.gcs` file
/// in Gem Cut Studio: if the crown facets are NOT above the pavilion facets there, the writer
/// reads the tooth of a pavilion facet the wrong way round (see the documentation of
/// `to_gcs_string`, "The open question").
///
/// Run it with `INDICATRIX_GCS_SAMPLE_DIR` set to an existing folder:
/// `cargo test -p indicatrix-formats write_the_sample_files_for_gem_cut_studio -- --ignored`.
#[test]
#[ignore = "writes sample files to INDICATRIX_GCS_SAMPLE_DIR for a check in Gem Cut Studio"]
fn write_the_sample_files_for_gem_cut_studio() {
    let dir = std::path::PathBuf::from(
        std::env::var_os("INDICATRIX_GCS_SAMPLE_DIR")
            .expect("set INDICATRIX_GCS_SAMPLE_DIR to an existing folder"),
    );
    let schedule = not_mirrored();
    let gcs = to_gcs_string(&schedule).expect("the schedule is a bounded stone");
    let asc = to_asc_string(&schedule).expect("the schedule is writable as .asc");
    std::fs::write(dir.join("chiral-check.gcs"), gcs).expect("the folder is writable");
    std::fs::write(dir.join("chiral-check.asc"), asc).expect("the folder is writable");
}
