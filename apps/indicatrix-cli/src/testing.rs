//! Fixtures and helpers for the tests. Compiled only for tests.

use crate::outcome::Outcome;
use indicatrix_cut_core::{ConstraintTier, Design, MaterialSelection, PreformSpec, ScheduleMeta};
use indicatrix_editor::templates::gallery_design;
use std::path::PathBuf;

/// The standard round brilliant template of the New Design gallery, in diamond.
pub fn template() -> Design {
    gallery_design(1)
}

/// The brilliant the Retarget engine's own tests use, as a 1.72 stone with no material name:
/// retargeting it to index 1.7681 with a third of the shift on the crown is known to give a
/// valid result with Shift alone.
pub fn brilliant_at_172() -> Design {
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

/// The name, angle and index positions of every tier: what must survive a file round trip.
pub fn shape(design: &Design) -> Vec<(String, f64, Vec<f64>)> {
    design
        .tiers
        .iter()
        .map(|tier| (tier.name.clone(), tier.angle_deg, tier.indices.clone()))
        .collect()
}

/// A path for `name` in a folder of this test process, which is created.
pub fn temp_path(name: &str) -> PathBuf {
    let folder = std::env::temp_dir().join(format!("indicatrix-cli-tests-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&folder);
    folder.join(name)
}

/// Writes `design` as a `.indicatrix` file called `name` and returns its path as text.
pub fn write_design(name: &str, design: &Design) -> String {
    let loaded = crate::load::Loaded::in_memory(design.clone(), name);
    let text = loaded
        .indicatrix_text(design, &crate::materials::Catalogue::default())
        .expect("the fixture can be written");
    let path = temp_path(name);
    std::fs::write(&path, text).expect("the fixture folder is writable");
    path.to_string_lossy().into_owned()
}

/// Runs the command line made of `parts`, in this process.
pub fn run(parts: &[&str]) -> Outcome {
    let args: Vec<String> = parts.iter().map(|part| (*part).to_string()).collect();
    crate::run_command_line(&args)
}

/// What `outcome` wrote to the file at `path`, if it wrote one.
pub fn file_text<'a>(outcome: &'a Outcome, path: &str) -> Option<&'a str> {
    outcome
        .files
        .iter()
        .find(|file| file.path.to_string_lossy() == path)
        .map(|file| file.text.as_str())
}
