//! Geometry-aware extension without changing the existing browser wire scene.

use std::sync::Arc;

use indicatrix::{
    geometry::tool::{MAX_TOOL_PRIMITIVES, ToolPrimitive},
    renderer::env_map::EnvironmentMap,
};

use super::{CustomMaterialSpec, OwnedScene, SceneSpec};

/// An immutable scene retaining the exact resolved cutting tools. Consumers that
/// cannot execute these tools must decline; converting to the planar spec loses geometry.
#[derive(Debug, Clone, PartialEq)]
pub struct GeometrySceneSpec {
    pub scene: SceneSpec,
    pub tools: Vec<ToolPrimitive>,
}

/// Checks a whole tool list: at most `MAX_TOOL_PRIMITIVES`, every tool valid. No partial
/// list is accepted. The scene build and the analysis Worker's metrics and tilt jobs use
/// this one check.
///
/// # Errors
///
/// The first problem, as a sentence.
pub fn validate_tools(tools: &[ToolPrimitive]) -> Result<(), String> {
    if tools.len() > MAX_TOOL_PRIMITIVES {
        return Err(format!(
            "{} tools exceed the supported limit of {MAX_TOOL_PRIMITIVES}",
            tools.len()
        ));
    }
    for (index, tool) in tools.iter().enumerate() {
        tool.validate()
            .map_err(|error| format!("Tool {}: {error}", index + 1))?;
    }
    Ok(())
}

impl GeometrySceneSpec {
    /// Validate all tools before building any traceable scene. No partial tool set
    /// is accepted on failure. Call on a preparation worker.
    ///
    /// # Errors
    ///
    /// The first tool problem, or the scene builder's error text.
    pub fn build(&self, hdr: Option<Arc<EnvironmentMap>>) -> Result<OwnedScene, String> {
        self.build_with_customs(hdr, &[])
    }

    /// [`Self::build`] for a caller that holds the session's [`CustomMaterialSpec`]s: the
    /// traced material's physics recipe fills the scene's fluorescence exactly as
    /// [`OwnedScene::build_with_customs`] does (the desktop's `active_fluorescence`). With no
    /// `customs`, or a traced material that is not among them, the scene is the one
    /// [`Self::build`] makes; the browser, which sends no recipes, is unchanged.
    ///
    /// # Errors
    ///
    /// The first tool problem, or the scene builder's error text.
    pub fn build_with_customs(
        &self,
        hdr: Option<Arc<EnvironmentMap>>,
        customs: &[CustomMaterialSpec],
    ) -> Result<OwnedScene, String> {
        validate_tools(&self.tools)?;
        let mut scene = OwnedScene::build_with_customs(&self.scene, hdr, customs)
            .map_err(|error| error.to_string())?;
        scene.tools.clone_from(&self.tools);
        Ok(scene)
    }
}

impl OwnedScene {
    /// Exact subtracted geometry, also required by backend eligibility checks.
    #[must_use]
    pub fn tools(&self) -> &[ToolPrimitive] {
        &self.tools
    }
}
