//! Fixtures the environment tests share: the concave fixture design retargeted to Sapphire.

use super::retarget_env::{RetargetEnv, RetargetEnvConfig};
use indicatrix_cut_core::{BuiltinMaterials, Design, MaterialSelection, ResolvedMaterial};

/// Sapphire as the retarget environments judge it (refractive index 1.77).
pub(super) fn sapphire() -> ResolvedMaterial {
    MaterialSelection {
        name: Some("Sapphire".to_string()),
        ..MaterialSelection::none()
    }
    .resolve(&BuiltinMaterials)
}

/// The concave fixture retargeted to Sapphire, with `configure` applied to the default
/// configuration first (four movable tiers, a closed stone that returns light).
pub(super) fn fixture_env_with(configure: impl FnOnce(&mut RetargetEnvConfig)) -> RetargetEnv {
    let mut config = RetargetEnvConfig::new_with_target(sapphire());
    configure(&mut config);
    RetargetEnv::new(Design::concave_fixture(), config)
        .expect("the fixture is a closed, movable stone")
}

/// [`fixture_env_with`] under the default configuration.
pub(super) fn fixture_env() -> RetargetEnv {
    fixture_env_with(|_| {})
}
