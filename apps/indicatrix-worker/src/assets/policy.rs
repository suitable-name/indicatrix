//! Where an HDR-lit request may run.
//!
//! The request loop (`crate::serve`'s `serve_requests`) asks [`hdr_route`]; the
//! capability advertisement (`crate::coordinator::viewer_render_capability`) asks
//! [`coordinator_advertises_hdr`].
//!
//! # Rules
//!
//! - A plain worker or a `join`ed worker serves an HDR scene exactly when it keeps an
//!   asset cache (it resolves the map itself, asking its peer -- a viewer, or the
//!   coordinator -- for missing bytes).
//! - A coordinator serves an HDR scene when it keeps an asset cache (it must hold the
//!   bytes for the job, to forward them) AND at least one of its render lanes renders
//!   HDR: its own lane (`--render`), or a joined worker whose `HELLO` capability says
//!   `hdr` (its cache opened). It advertises `RenderCapability::hdr` by the same rule.
//! - Lane selection for an HDR job only takes joined workers advertising `hdr`
//!   (`crate::coordinator`'s `LaneNeed`); workers without it still serve studio jobs.
//!
//! A refusal is `UNSUPPORTED_REQUEST`, so a viewer falls back to local rendering.

use super::AssetCache;
use crate::coordinator::{Capacity, ViewerSession};
use indicatrix_net::{
    SceneState,
    messages::{ErrorMsg, RenderCapability, error_codes},
};

/// What [`hdr_route`] decided for one request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HdrRoute {
    /// The scene is lit by the studio rig: nothing to resolve.
    NotHdr,
    /// Resolve the map (`super::ensure_environment`, or `super::ensure_held` on a
    /// coordinator), then serve as usual.
    Serve,
    /// Refuse the request with this error.
    Refuse(ErrorMsg),
}

/// The rule for `scene` on this connection -- see the module doc comment.
///
/// `assets` is this connection's cache (`None` when the server keeps none); on a
/// coordinator's viewer connection (`session`) the coordinator's own cache decides.
#[must_use]
pub fn hdr_route(
    scene: &SceneState,
    assets: Option<&AssetCache>,
    session: Option<&ViewerSession>,
) -> HdrRoute {
    if scene.hdr().is_none() {
        return HdrRoute::NotHdr;
    }
    let Some(session) = session else {
        return if assets.is_some() {
            HdrRoute::Serve
        } else {
            refuse("this server does not render HDR environments (it keeps no asset cache)")
        };
    };
    let coordinator = &session.coordinator;
    if coordinator.assets().is_none() {
        return refuse(
            "this coordinator keeps no HDR asset cache (see INDICATRIX_ASSET_CACHE_DIR), so it cannot \
             hold an HDR map for its render lanes",
        );
    }
    let hdr_workers = coordinator
        .registry()
        .map_or(0, |registry| registry.capacity().hdr_workers);
    if coordinator.own().is_some() || hdr_workers > 0 {
        return HdrRoute::Serve;
    }
    refuse(
        "no render lane of this coordinator renders HDR environments: it has no own lane (--render) and \
         no joined worker with an asset cache",
    )
}

/// Whether a coordinator with joined `workers` advertises `hdr` to viewers: it holds an
/// asset cache (`holds_assets`) and its own lane renders HDR (`own.hdr`) or at least one
/// joined worker does.
#[must_use]
pub fn coordinator_advertises_hdr(
    own: Option<&RenderCapability>,
    workers: Capacity,
    holds_assets: bool,
) -> bool {
    holds_assets && (own.is_some_and(|own| own.hdr) || workers.hdr_workers > 0)
}

fn refuse(message: &str) -> HdrRoute {
    HdrRoute::Refuse(ErrorMsg {
        code: error_codes::UNSUPPORTED_REQUEST,
        message: message.to_string(),
    })
}
