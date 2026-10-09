//! The one rule deciding whether a remote may contribute samples to an image at all
//! ([`remote_can_render`]), and the live viewport's per-settle dispatch decision built
//! on it ([`live_remote_dispatch`]).
//!
//! # Why this is a correctness rule, not a performance one
//!
//! Merging backends is only sound when every backend renders the IDENTICAL scene (see
//! the hybrid guide's invariant 2): per-pixel sums of different scenes are a physically
//! meaningless composite, not a noisier estimate of either.
//!
//! Since protocol v14 an HDR-lit scene travels by content hash: a remote that
//! advertises `RenderCapability::hdr` fetches the map's bytes (`NEED_ASSET`) and decodes
//! them with the same builder the viewer uses, so its samples are lit identically. The
//! rule is therefore PER CAPABILITY: an HDR scene may go to a remote exactly when that
//! remote renders HDR and the map can be sent at all (it was loaded from a file, see
//! `super::hdr_asset`, within the protocol's size limit). Every remote path -- the live
//! view, the still export, the tilt video, batches -- asks this module instead of
//! keeping its own ad-hoc check.

use super::hdr_asset;
use crate::settings::LiveComputeTarget;
use indicatrix::renderer::env_map::EnvironmentMap;
use std::sync::Arc;

/// Why a remote must not render a given scene.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteRefusal {
    /// The scene is lit by an HDR environment map and the remote does not advertise HDR
    /// support (`RenderCapability::hdr`); it would light the stone with the studio rig.
    HdrEnvironment,
    /// The scene's HDR map cannot be sent: it was not loaded from a file (no bytes to
    /// transfer) or exceeds the protocol's asset size limit.
    HdrNotTransferable,
    /// `zoning` builds: the scene's material has colour zones and the remote does not
    /// advertise the zoning capability (a default build, or a coordinator); it would render
    /// only the base zone, so the picture renders locally.
    #[cfg(feature = "zoning")]
    NoZoningSupport,
}

impl RemoteRefusal {
    /// The one-time status note the live viewport shows when it falls back to local
    /// rendering for this reason.
    #[must_use]
    pub const fn live_note(self) -> &'static str {
        match self {
            Self::HdrEnvironment => {
                "This remote doesn't support HDR environments; rendering locally"
            }
            Self::HdrNotTransferable => {
                "This HDR environment can't be sent to the remote; rendering locally"
            }
            #[cfg(feature = "zoning")]
            Self::NoZoningSupport => "Worker has no zoning support; rendering locally",
        }
    }

    /// The note an export (still or tilt video) shows when it falls back to local-only
    /// rendering for this reason.
    #[must_use]
    pub const fn export_note(self) -> &'static str {
        match self {
            Self::HdrEnvironment => {
                "This export uses an HDR environment map, which the remote does not \
                 support -- rendering locally only so the whole image is lit by the \
                 same environment."
            }
            Self::HdrNotTransferable => {
                "This export uses an HDR environment map that cannot be sent to the \
                 remote -- rendering locally only so the whole image is lit by the same \
                 environment."
            }
            #[cfg(feature = "zoning")]
            Self::NoZoningSupport => {
                "This stone has colour zones, which the remote does not support (worker has \
                 no zoning support) -- rendering locally only so the whole image shows the \
                 same zones."
            }
        }
    }
}

/// `zoning` builds: whether a remote may contribute samples to a scene whose material is
/// `zoned` (has colour zones), given whether the remote advertised the zoning capability
/// (`remote_zoning`, `Welcome::zoning`). A zoned scene sent to a remote without the
/// capability would render as its base zone only and be summed into the same buffer as
/// the correctly zoned local half -- unsound for the same reason as an HDR mismatch (see the
/// module doc comment), so it keeps local. An unzoned scene is never refused.
///
/// The connection layer applies the same rule against the live `WELCOME`
/// (`RemoteError::ZoningUnsupported`), so a path that cannot call this up front (the live view
/// before its first connection, the single-picture preview) still never sends a zoned scene to a
/// peer that cannot decode its payload.
///
/// # Errors
///
/// [`RemoteRefusal::NoZoningSupport`] for a zoned scene and a remote without the capability.
#[cfg(feature = "zoning")]
pub const fn zoned_scene_refusal(zoned: bool, remote_zoning: bool) -> Result<(), RemoteRefusal> {
    if zoned && !remote_zoning {
        Err(RemoteRefusal::NoZoningSupport)
    } else {
        Ok(())
    }
}

/// Whether a remote may contribute samples to a scene whose environment is `env_map`
/// (`None` = the analytic studio rig), given whether that remote renders HDR scenes
/// (`remote_hdr`, its `RenderCapability::hdr`). See the module doc comment.
///
/// # Errors
///
/// [`RemoteRefusal::HdrNotTransferable`] when the HDR map cannot be sent at all;
/// [`RemoteRefusal::HdrEnvironment`] when the remote does not render HDR scenes.
pub fn remote_can_render(
    env_map: Option<&Arc<EnvironmentMap>>,
    remote_hdr: bool,
) -> Result<(), RemoteRefusal> {
    let Some(map) = env_map else {
        return Ok(());
    };
    if !hdr_asset::asset_for(map).is_some_and(|asset| asset.transferable()) {
        return Err(RemoteRefusal::HdrNotTransferable);
    }
    if remote_hdr {
        Ok(())
    } else {
        Err(RemoteRefusal::HdrEnvironment)
    }
}

/// What the live viewport should do at a settle, decided from the live compute target,
/// whether a worker is configured, and the scene's environment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LiveDispatch {
    /// Dispatch remote work for this settle (`Both` or `RemoteOnly` with a worker).
    Remote,
    /// Render locally: `LocalOnly`, or no worker configured.
    Local,
    /// Render locally because remote may not render this scene. `Both` behaves like
    /// `LocalOnly`; `RemoteOnly` renders locally too (never a blank viewport).
    Refused(RemoteRefusal),
}

/// The live viewport's per-settle dispatch decision -- pure, so it is unit-tested
/// without a window or socket.
///
/// `remote_hdr` is the remote's `RenderCapability::hdr` as last seen on the live
/// connection, `None` before it ever connected: an unknown capability does not block
/// the first dispatch, because the connection layer refuses to send an HDR scene to a
/// remote whose `WELCOME` says it cannot render one (and the answer is remembered for
/// the next settle).
#[must_use]
pub fn live_remote_dispatch(
    target: LiveComputeTarget,
    worker_configured: bool,
    env_map: Option<&Arc<EnvironmentMap>>,
    remote_hdr: Option<bool>,
) -> LiveDispatch {
    if matches!(target, LiveComputeTarget::LocalOnly) || !worker_configured {
        return LiveDispatch::Local;
    }
    match remote_can_render(env_map, remote_hdr.unwrap_or(true)) {
        Ok(()) => LiveDispatch::Remote,
        Err(refusal) => LiveDispatch::Refused(refusal),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A map loaded from a real (temporary) `.hdr` file, so it has a transferable asset.
    fn loaded_hdr() -> Arc<EnvironmentMap> {
        let pixels = vec![image::Rgb([0.5f32, 0.5, 0.5]); 32];
        let mut bytes = Vec::new();
        image::codecs::hdr::HdrEncoder::new(&mut bytes)
            .encode(&pixels, 8, 4)
            .unwrap();
        let path = std::env::temp_dir().join(format!(
            "indicatrix-cut-guard-{}-{:?}.hdr",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::write(&path, &bytes).unwrap();
        let (map, _asset) = hdr_asset::load_hdr_file(&path).unwrap();
        let _ = std::fs::remove_file(&path);
        map
    }

    /// A map built in memory: no asset, nothing to send.
    fn synthetic_hdr() -> Arc<EnvironmentMap> {
        Arc::new(EnvironmentMap::uniform(4, 4, [0.5, 0.5, 0.5]))
    }

    #[test]
    fn an_hdr_scene_may_render_remotely_exactly_when_the_remote_supports_hdr() {
        let map = loaded_hdr();
        assert_eq!(remote_can_render(None, false), Ok(()));
        assert_eq!(remote_can_render(None, true), Ok(()));
        assert_eq!(remote_can_render(Some(&map), true), Ok(()));
        assert_eq!(
            remote_can_render(Some(&map), false),
            Err(RemoteRefusal::HdrEnvironment)
        );
        // A map with no source bytes can never be sent, whatever the remote supports.
        for remote_hdr in [false, true] {
            assert_eq!(
                remote_can_render(Some(&synthetic_hdr()), remote_hdr),
                Err(RemoteRefusal::HdrNotTransferable)
            );
        }
    }

    #[test]
    fn the_live_view_dispatches_an_hdr_scene_only_to_an_hdr_capable_remote() {
        let map = loaded_hdr();
        for target in [LiveComputeTarget::RemoteOnly, LiveComputeTarget::Both] {
            assert_eq!(
                live_remote_dispatch(target, true, Some(&map), Some(false)),
                LiveDispatch::Refused(RemoteRefusal::HdrEnvironment),
                "{target:?}, remote without HDR"
            );
            assert_eq!(
                live_remote_dispatch(target, true, Some(&map), Some(true)),
                LiveDispatch::Remote,
                "{target:?}, HDR-capable remote"
            );
            // Not yet connected: dispatch; the connection layer checks the WELCOME.
            assert_eq!(
                live_remote_dispatch(target, true, Some(&map), None),
                LiveDispatch::Remote
            );
            assert_eq!(
                live_remote_dispatch(target, true, Some(&synthetic_hdr()), Some(true)),
                LiveDispatch::Refused(RemoteRefusal::HdrNotTransferable)
            );
        }
        assert_eq!(
            live_remote_dispatch(LiveComputeTarget::LocalOnly, true, Some(&map), Some(true)),
            LiveDispatch::Local
        );
    }

    #[test]
    fn a_studio_scene_dispatches_only_with_a_worker_and_a_remote_mode() {
        for remote_hdr in [None, Some(false), Some(true)] {
            assert_eq!(
                live_remote_dispatch(LiveComputeTarget::Both, true, None, remote_hdr),
                LiveDispatch::Remote
            );
            assert_eq!(
                live_remote_dispatch(LiveComputeTarget::RemoteOnly, true, None, remote_hdr),
                LiveDispatch::Remote
            );
            assert_eq!(
                live_remote_dispatch(LiveComputeTarget::LocalOnly, true, None, remote_hdr),
                LiveDispatch::Local
            );
            assert_eq!(
                live_remote_dispatch(LiveComputeTarget::Both, false, None, remote_hdr),
                LiveDispatch::Local
            );
        }
    }

    /// A zoned scene may go to a remote exactly when that remote advertised the zoning
    /// capability; an unzoned scene is never refused.
    #[cfg(feature = "zoning")]
    #[test]
    fn a_zoned_scene_renders_remotely_only_on_a_zoning_remote() {
        assert_eq!(zoned_scene_refusal(false, false), Ok(()));
        assert_eq!(zoned_scene_refusal(false, true), Ok(()));
        assert_eq!(zoned_scene_refusal(true, true), Ok(()));
        assert_eq!(
            zoned_scene_refusal(true, false),
            Err(RemoteRefusal::NoZoningSupport)
        );
        assert!(
            RemoteRefusal::NoZoningSupport
                .live_note()
                .contains("no zoning support")
        );
        assert!(
            RemoteRefusal::NoZoningSupport
                .export_note()
                .contains("no zoning support")
        );
    }

    #[test]
    fn the_live_note_is_the_agreed_wording() {
        assert_eq!(
            RemoteRefusal::HdrEnvironment.live_note(),
            "This remote doesn't support HDR environments; rendering locally"
        );
        assert!(RemoteRefusal::HdrEnvironment.export_note().contains("HDR"));
        assert!(
            RemoteRefusal::HdrNotTransferable
                .export_note()
                .contains("HDR")
        );
    }
}
