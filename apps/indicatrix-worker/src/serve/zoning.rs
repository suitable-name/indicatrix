//! Zones received over the wire (`zoning` feature): the per-connection stash of
//! `ClientMessage::ZoningPayload`s and the step that re-attaches them to the scene they
//! belong to.
//!
//! A zoned material serialises WITHOUT its zones (`GemMaterial::zoning` is `serde(skip)`), so
//! the viewer sends them first, as a [`ZoningPayload`] keyed by the request id, and the
//! request after. Wherever this worker reads a `ClientMessage` -- the idle request loop
//! (`connection::requests`), the streaming emitter's poll, the HDR asset wait, the batch
//! session, the tilt-curves poll -- a payload is [`stash`]ed here, and the request that
//! follows [`take`]s it before it is validated and traced.
//!
//! # Why a thread-local
//!
//! Every one of those readers runs on the connection's own thread (the emitter is the
//! connection thread, with the tracer on a helper), and threading a stash through all of their
//! signatures would touch every default code path for a feature that is off by default. The
//! stash is bounded ([`MAX_STASHED`]), keyed by request id, consumed on use, and emptied when a
//! connection starts and ends ([`ConnectionGuard`]), so a payload can never leak into another
//! connection served by the same thread.
//!
//! # What the worker cannot know
//!
//! The scene is unchanged on the wire, so a scene that SHOULD have been zoned but arrives without
//! its payload cannot be told from an unzoned one: it renders as its base zone. The viewer
//! guarantees the order (payload immediately before the request, only to a peer that advertised
//! the capability); the worker enforces what it can -- the payload must validate, the material
//! must be per-millimetre, the scene must not be fluorescent, the payload must name what the
//! request has.

use indicatrix_net::{
    SceneState,
    messages::{BatchRenderRequest, ZoningPayload},
};
use std::{cell::RefCell, collections::BTreeMap};

/// How many unclaimed payloads one connection may hold; the lowest request id is dropped
/// first.
pub const MAX_STASHED: usize = 8;

thread_local! {
    static STASH: RefCell<BTreeMap<u32, ZoningPayload>> = const { RefCell::new(BTreeMap::new()) };
}

/// Keeps `payload` for the request it names (replacing an earlier one for the same id).
pub fn stash(payload: ZoningPayload) {
    STASH.with(|stash| {
        let mut stash = stash.borrow_mut();
        stash.insert(payload.request_id, payload);
        while stash.len() > MAX_STASHED {
            stash.pop_first();
        }
    });
}

/// Removes and returns the payload for `request_id`.
pub fn take(request_id: u32) -> Option<ZoningPayload> {
    STASH.with(|stash| stash.borrow_mut().remove(&request_id))
}

/// Drops every stashed payload.
pub fn clear() {
    STASH.with(|stash| stash.borrow_mut().clear());
}

/// Empties the stash when a connection starts and again when it ends.
pub struct ConnectionGuard;

impl ConnectionGuard {
    /// Starts a connection's scope.
    pub(crate) fn new() -> Self {
        clear();
        Self
    }
}

impl Drop for ConnectionGuard {
    fn drop(&mut self) {
        clear();
    }
}

/// Installs the stashed zones of `request_id` on `scene`, if any were sent.
///
/// `Ok(true)` when zones were attached, `Ok(false)` when no payload exists (an unzoned
/// request).
///
/// # Errors
///
/// A human-readable reason when the payload does not fit the scene (invalid zones, a material
/// that is not per-millimetre, fluorescence, a batch-shaped payload on a single scene). The
/// payload is consumed either way and `scene` is left untouched on error.
pub fn attach_to_render(request_id: u32, scene: &mut SceneState) -> Result<bool, String> {
    let Some(payload) = take(request_id) else {
        return Ok(false);
    };
    payload
        .attach_to_scene(scene)
        .map_err(|e| format!("request {request_id}: {e}"))?;
    Ok(true)
}

/// Installs the stashed zones of the batch `request.request_id` on its items, if any were
/// sent; returns the request with the zones attached.
///
/// # Errors
///
/// `(request_id, reason)` when the payload does not fit the batch; the whole batch is then
/// refused (a half-zoned batch would silently render some items as their base zone).
pub fn attach_to_batch(
    mut request: BatchRenderRequest,
) -> Result<BatchRenderRequest, (u32, String)> {
    let request_id = request.request_id;
    let Some(payload) = take(request_id) else {
        return Ok(request);
    };
    payload
        .attach_to_batch(&mut request.items)
        .map_err(|e| (request_id, format!("batch {request_id}: {e}")))?;
    Ok(request)
}

#[cfg(test)]
mod tests {
    use super::*;
    use indicatrix::{
        geometry::cuts::StandardGemCuts,
        optics::{
            absorption::AbsorptionTensor,
            fluorescence::Fluorescence,
            materials::GemMaterial,
            raytracer::LightingPreset,
            zoning::{Zone, ZoneAbsorption, ZoneShape, ZonedAbsorption},
        },
    };
    use indicatrix_net::messages::BatchItem;

    /// Whether a payload for `request_id` is waiting.
    fn has(request_id: u32) -> bool {
        STASH.with(|stash| stash.borrow().contains_key(&request_id))
    }

    fn zones() -> ZonedAbsorption {
        let mut zoned =
            ZonedAbsorption::new(ZoneAbsorption::per_mm(AbsorptionTensor::isotropic(vec![])));
        zoned.zones.push(Zone {
            shape: ZoneShape::HalfSpace {
                normal: glam::DVec3::X,
                offset: 0.0,
            },
            absorption: ZoneAbsorption::per_mm(AbsorptionTensor::isotropic(vec![])),
        });
        zoned
    }

    fn scene(zoned: bool) -> SceneState {
        let mut material = GemMaterial::diamond();
        if zoned {
            material = material.with_zoning(zones());
        }
        SceneState {
            width: 8,
            height: 8,
            yaw: 0.4,
            pitch: 0.3,
            distance: 3.0,
            light_yaw: 0.85,
            light_pitch: 0.95,
            exposure: 1.0,
            max_bounces: 4,
            lighting_preset: LightingPreset::Daylight,
            material,
            planes: StandardGemCuts::standard_round_brilliant(),
            girdle_frosted: false,
            backdrop: 0.0,
            environment: indicatrix_net::scene::SceneEnvironment::Studio,
            surface_glare: 1.0,
            tools: Vec::new(),
            fluorescence: Fluorescence::default(),
            head_shadow_deg: 16.0,
        }
    }

    /// What the worker holds after decoding: the scene without its zones.
    fn received(scene: &SceneState) -> SceneState {
        let bytes = postcard::to_allocvec(scene).unwrap();
        postcard::from_bytes(&bytes).unwrap()
    }

    #[test]
    fn a_stashed_payload_is_attached_once_and_then_gone() {
        let _guard = ConnectionGuard::new();
        let local = scene(true);
        stash(ZoningPayload::for_scene(7, &local).unwrap());
        assert!(has(7));

        let mut wire_scene = received(&local);
        assert!(wire_scene.material.zoning.is_none());
        assert_eq!(attach_to_render(7, &mut wire_scene), Ok(true));
        assert_eq!(wire_scene, local);

        assert!(!has(7));
        let mut other = received(&local);
        assert_eq!(attach_to_render(7, &mut other), Ok(false));
        assert!(other.material.zoning.is_none());
    }

    #[test]
    fn an_unfitting_payload_is_refused_with_a_reason() {
        let _guard = ConnectionGuard::new();
        let local = scene(true);
        let mut payload = ZoningPayload::for_scene(3, &local).unwrap();
        // Five extra zones: over the four-zone limit.
        let extra = payload.materials[0].zoning.zones[0].clone();
        for _ in 0..5 {
            payload.materials[0].zoning.zones.push(extra.clone());
        }
        stash(payload);
        let mut wire_scene = received(&local);
        let err = attach_to_render(3, &mut wire_scene).unwrap_err();
        assert!(err.contains("request 3"), "{err}");
        assert!(wire_scene.material.zoning.is_none());
    }

    #[test]
    fn a_batch_gets_its_zones_per_item_and_a_bad_payload_refuses_the_whole_batch() {
        let _guard = ConnectionGuard::new();
        let items = vec![
            BatchItem {
                item_id: 1,
                scene: scene(true),
                first_sample: 0,
                samples: 4,
                width: 8,
                height: 8,
            },
            BatchItem {
                item_id: 2,
                scene: scene(false),
                first_sample: 0,
                samples: 4,
                width: 8,
                height: 8,
            },
        ];
        stash(ZoningPayload::for_batch(5, &items).unwrap());
        let wire = BatchRenderRequest {
            request_id: 5,
            reply: indicatrix_net::messages::BatchReply::FinalPng,
            items: items
                .iter()
                .map(|item| BatchItem {
                    scene: received(&item.scene),
                    ..item.clone()
                })
                .collect(),
        };
        let attached = attach_to_batch(wire.clone()).unwrap();
        assert_eq!(attached.items, items);

        // A payload naming an item the batch lacks refuses the batch.
        let mut stray = ZoningPayload::for_batch(6, &items).unwrap();
        stray.materials[0].item_id = Some(99);
        stash(stray);
        let mut other = wire;
        other.request_id = 6;
        let (id, reason) = attach_to_batch(other).unwrap_err();
        assert_eq!(id, 6);
        assert!(reason.contains("99"), "{reason}");
    }

    #[test]
    fn the_stash_is_bounded_and_a_connection_starts_and_ends_empty() {
        let guard = ConnectionGuard::new();
        let local = scene(true);
        let max = u32::try_from(MAX_STASHED).unwrap();
        for id in 0..(max + 4) {
            stash(ZoningPayload::for_scene(id, &local).unwrap());
        }
        assert!(!has(0), "the lowest ids were dropped");
        assert!(has(max + 3));
        drop(guard);
        assert!(!has(max + 3), "the guard empties it on drop");
        stash(ZoningPayload::for_scene(1, &local).unwrap());
        let _next = ConnectionGuard::new();
        assert!(!has(1), "a new connection never sees an old payload");
    }
}
