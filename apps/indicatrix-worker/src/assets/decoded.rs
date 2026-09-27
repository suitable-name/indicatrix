//! The process-wide registry of DECODED environment maps, keyed by content hash, and the
//! one way the tracer turns a scene into its `EnvironmentSource` ([`environment_source`]).
//!
//! # Lifetimes
//!
//! - A request that resolved its map holds an `Arc` for as long as it streams, so the
//!   tracer thread's [`resolved_hdr_map`] always finds it.
//! - The [`RECENT_KEEP`] most recently resolved maps stay alive between requests: a live
//!   view sends a new request per settle, and re-decoding a large panorama each time
//!   would dominate the latency.
//! - Every map ever registered also leaves a `Weak` behind for the life of the process.
//!   The GPU backend recognises "the same map as last dispatch" by its address; a `Weak`
//!   keeps that allocation reserved (the texels themselves are freed), so a later,
//!   different map can never reuse the address and be mistaken for a stale upload.
//!
//! # Decoding
//!
//! Only through `indicatrix::renderer::env_map::environment_from_hdr_bytes` with
//! `HdrLimits::DEFAULT` -- the viewer's own builder, so both sides hold the identical map
//! -- and one decode at a time per process ([`DECODE_LOCK`]), bounding peak memory to one
//! map's decode however many connections ask at once.

use indicatrix::{
    optics::raytracer::EnvironmentSource,
    renderer::env_map::{EnvironmentMap, HdrLimits, environment_from_hdr_bytes},
};
use indicatrix_net::{
    SceneState,
    messages::{ContentHash, hash_hex},
    scene::HdrEnvironment,
};
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex, PoisonError, Weak},
};

/// How many recently resolved maps stay decoded between requests.
const RECENT_KEEP: usize = 2;

/// Every map ever registered, by hash (see the module doc comment's "Lifetimes").
static REGISTERED: Mutex<Vec<(ContentHash, Weak<EnvironmentMap>)>> = Mutex::new(Vec::new());

/// The maps kept decoded between requests, most recent last.
static RECENT: Mutex<VecDeque<(ContentHash, Arc<EnvironmentMap>)>> = Mutex::new(VecDeque::new());

/// Serialises decodes process-wide (see the module doc comment's "Decoding").
static DECODE_LOCK: Mutex<()> = Mutex::new(());

/// The decoded map for `hash`, if one is alive (pinned by a request or kept recent).
#[must_use]
pub fn lookup(hash: &ContentHash) -> Option<Arc<EnvironmentMap>> {
    let found = REGISTERED
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .iter()
        .rev()
        .filter(|(h, _)| h == hash)
        .find_map(|(_, weak)| weak.upgrade());
    if let Some(map) = &found {
        remember_recent(hash, map);
    }
    found
}

/// Decodes `bytes` (the asset `expected` names) with the shared builder and registers
/// the result. The decoded dimensions must equal `expected`'s.
///
/// # Errors
///
/// A human-readable reason when the bytes do not decode (malformed, over
/// `HdrLimits::DEFAULT`) or decode to other dimensions than the scene declared.
pub fn decode_and_register(
    expected: &HdrEnvironment,
    bytes: &[u8],
) -> Result<Arc<EnvironmentMap>, String> {
    let map = {
        let _one_at_a_time = DECODE_LOCK.lock().unwrap_or_else(PoisonError::into_inner);
        environment_from_hdr_bytes(bytes, HdrLimits::DEFAULT).map_err(|e| e.to_string())?
    };
    if (map.width(), map.height()) != (expected.width as usize, expected.height as usize) {
        return Err(format!(
            "the HDR map decodes to {}x{} texels but the scene declared {}x{}",
            map.width(),
            map.height(),
            expected.width,
            expected.height
        ));
    }
    let map = Arc::new(map);
    REGISTERED
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .push((expected.content_hash, Arc::downgrade(&map)));
    remember_recent(&expected.content_hash, &map);
    Ok(map)
}

/// Moves `hash` to the most-recent end of [`RECENT`], dropping the oldest past
/// [`RECENT_KEEP`].
fn remember_recent(hash: &ContentHash, map: &Arc<EnvironmentMap>) {
    push_recent(
        &mut RECENT.lock().unwrap_or_else(PoisonError::into_inner),
        hash,
        map,
    );
}

/// [`remember_recent`] on the locked list.
fn push_recent(
    recent: &mut VecDeque<(ContentHash, Arc<EnvironmentMap>)>,
    hash: &ContentHash,
    map: &Arc<EnvironmentMap>,
) {
    recent.retain(|(h, _)| h != hash);
    recent.push_back((*hash, Arc::clone(map)));
    while recent.len() > RECENT_KEEP {
        recent.pop_front();
    }
}

/// The decoded HDR map `scene` is lit by, or `None` for the studio rig.
///
/// # Panics
///
/// When `scene` names an HDR map that no request resolved first. The request path
/// (`crate::serve`'s request loop, via `super::fetch`) always resolves and pins the map
/// before tracing starts; a panic here means a caller skipped that, and the tracer's
/// `catch_unwind` turns it into a `TRACE_PANIC` error rather than a studio-lit image --
/// silently lighting an HDR scene with the studio rig is exactly the bug this guards.
#[must_use]
pub fn resolved_hdr_map(scene: &SceneState) -> Option<Arc<EnvironmentMap>> {
    let hdr = scene.hdr()?;
    Some(lookup(&hdr.content_hash).unwrap_or_else(|| {
        panic!(
            "HDR environment {} was not resolved before tracing (the request path must fetch it first)",
            hash_hex(&hdr.content_hash)
        )
    }))
}

/// The `EnvironmentSource` the tracer lights `scene` with.
///
/// The studio rig from the scene's lighting fields, or `hdr_map` (from
/// [`resolved_hdr_map`]) exactly as the viewer lights an HDR scene
/// (`EnvironmentSource::HdrMap`, no backdrop).
#[must_use]
pub fn environment_source<'a>(
    scene: &SceneState,
    hdr_map: Option<&'a EnvironmentMap>,
) -> EnvironmentSource<'a> {
    hdr_map.map_or_else(
        || {
            scene
                .lighting_preset
                .studio(scene.exposure, scene.light_yaw, scene.light_pitch)
                .with_backdrop(scene.backdrop)
        },
        EnvironmentSource::HdrMap,
    )
}
