//! The zoning wire extension (`zoning` feature only).
//!
//! Colour zones travel BESIDE the scene, never inside it, and the protocol version stays 24.
//!
//! # Why a separate message
//!
//! [`indicatrix::optics::materials::GemMaterial::zoning`] is `serde(skip)`: a zoned material
//! serialises exactly like an unzoned one (its `absorption` is the base zone's tensor), so
//! `SceneState`, `BatchItem` and every pinned byte of a default build are untouched. The
//! zones follow in a [`ZoningPayload`] (`ClientMessage::ZoningPayload`, enum index 9, appended
//! last), written immediately BEFORE the request it belongs to; the receiving worker keeps it
//! by `request_id` and re-attaches the zones to the decoded scene (`ZoningPayload::attach_to_scene`
//! / `attach_to_batch`).
//!
//! # The capability bit, and the proof a default build never sees it
//!
//! postcard is not self-describing and `Hello`/`Welcome` have no flags field, so the bit is a
//! TRAILING marker ([`ZONING_TAIL`], three bytes) after the encoded struct:
//!
//! 1. A zoning viewer writes `HELLO` + marker ([`write_hello_message`]).
//! 2. A worker's `read_hello` decodes with `postcard::from_bytes`, which ignores trailing
//!    bytes (postcard 1.1.3, `de::from_bytes` returns after the value without checking the
//!    remainder), so a DEFAULT worker pairs with a zoning viewer exactly as before and replies
//!    with a plain `WELCOME`.
//! 3. A zoning worker replies with `WELCOME` + marker ONLY when it saw the marker in the
//!    `HELLO` ([`write_welcome_message`]). A default viewer never sends the marker, so it never
//!    receives one: its handshake (which rejects any trailing byte after `WELCOME`) is
//!    unaffected.
//! 4. A zoning viewer strips the marker with [`split_welcome_tail`] and then decodes as usual:
//!    no marker means "no zoning support" (a default worker), the marker means "supported", any
//!    other trailing byte is refused as before.
//!
//! So the only bytes that differ between builds are the marker, and it appears only between two
//! zoning builds, or on a `HELLO` that a default worker provably ignores. A coordinator never
//! advertises the bit (zoned pictures through it render locally), and a zoning worker connection
//! that is a coordinator's viewer connection answers without it.

use super::{Hello, NetError, Welcome};
use crate::{framing, scene::SceneState};
use indicatrix::optics::{
    materials::AbsorptionUnit,
    zoning::{ZonedAbsorption, ZoningError},
};
use serde::{Deserialize, Serialize};
use std::io::Write;

/// The capability marker appended after a `HELLO` / `WELCOME` by a zoning build.
///
/// It is `"ZN"` and a marker version. See the module doc comment.
pub const ZONING_TAIL: [u8; 3] = [0x5A, 0x4E, 0x01];

/// The most materials one [`ZoningPayload`] carries (one per batch item, so
/// [`super::MAX_BATCH_ITEMS`]).
pub const MAX_ZONED_ENTRIES: usize = super::MAX_BATCH_ITEMS;

/// One zoned material of a [`ZoningPayload`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ZonedMaterialEntry {
    /// `None`: the material of the request's single scene (`RenderRequest`,
    /// `FinalImageRequest`). `Some(id)`: the material of `BatchItem::item_id == id` in a
    /// `BatchRenderRequest`.
    pub item_id: Option<u32>,
    /// The zones to install on that scene's material (`GemMaterial::zoning`).
    pub zoning: ZonedAbsorption,
}

/// `-> ZONING_PAYLOAD`: the zones of the request that follows, keyed by its `request_id`.
///
/// Sent only to a peer that advertised the zoning capability. It carries no reply of its own.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ZoningPayload {
    /// The `request_id` of the `RenderRequest` / `FinalImageRequest` /
    /// `BatchRenderRequest` this payload belongs to.
    pub request_id: u32,
    /// The zoned materials, in a fixed order (single scene first, then batch items by
    /// ascending id).
    pub materials: Vec<ZonedMaterialEntry>,
}

/// Why a [`ZoningPayload`] cannot be applied.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ZoningPayloadError {
    /// The payload names more than [`MAX_ZONED_ENTRIES`] materials.
    TooManyEntries {
        /// How many it named.
        count: usize,
    },
    /// The payload has no entries.
    Empty,
    /// The same target appears twice.
    DuplicateTarget {
        /// The repeated target (`None` = the single scene).
        item_id: Option<u32>,
    },
    /// A single-scene payload (no `item_id`) was applied to a batch, or a batch payload to a
    /// single scene.
    WrongShape,
    /// An entry names a batch item the request does not have.
    UnknownItem {
        /// The missing item id.
        item_id: u32,
    },
    /// The zones fail [`ZonedAbsorption::validate`].
    Invalid {
        /// The target whose zones are invalid.
        item_id: Option<u32>,
        /// The validation failure.
        error: ZoningError,
    },
    /// The scene's material is not `PerMm`; zone lengths are millimetres.
    MaterialNotPerMm {
        /// The offending target.
        item_id: Option<u32>,
    },
    /// The scene has fluorescent emitters; they are not supported on a zoned material.
    FluorescentScene {
        /// The offending target.
        item_id: Option<u32>,
    },
}

impl std::fmt::Display for ZoningPayloadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        fn target(item_id: Option<u32>) -> String {
            item_id.map_or_else(|| "the scene".to_string(), |id| format!("batch item {id}"))
        }
        match self {
            Self::TooManyEntries { count } => write!(
                f,
                "zoning payload names {count} materials (at most {MAX_ZONED_ENTRIES})"
            ),
            Self::Empty => write!(f, "zoning payload names no materials"),
            Self::DuplicateTarget { item_id } => {
                write!(f, "zoning payload names {} twice", target(*item_id))
            }
            Self::WrongShape => write!(
                f,
                "zoning payload shape does not match the request (single scene versus batch)"
            ),
            Self::UnknownItem { item_id } => {
                write!(
                    f,
                    "zoning payload names batch item {item_id}, which the request lacks"
                )
            }
            Self::Invalid { item_id, error } => {
                write!(f, "zones for {} are invalid: {error}", target(*item_id))
            }
            Self::MaterialNotPerMm { item_id } => write!(
                f,
                "the material of {} is not per-millimetre, so it cannot carry zones",
                target(*item_id)
            ),
            Self::FluorescentScene { item_id } => write!(
                f,
                "{} has fluorescent emitters, which zoned materials do not support",
                target(*item_id)
            ),
        }
    }
}

impl std::error::Error for ZoningPayloadError {}

/// Checks that `zoning` may be installed on `scene`'s material.
///
/// The zones validate, the material is per-millimetre (zone lengths are millimetres) and the
/// scene is not fluorescent.
///
/// Installing is then just `scene.material.zoning = Some(zoning.clone())`: the material's
/// `absorption` / `absorption_unit` are left as they arrived. The viewer built the material with
/// `GemMaterial::with_zoning`, so they already hold the base zone (the only thing that crosses
/// the wire), and the sizing step has already run on the viewer's side.
fn check(
    scene: &SceneState,
    zoning: &ZonedAbsorption,
    item_id: Option<u32>,
) -> Result<(), ZoningPayloadError> {
    zoning
        .validate()
        .map_err(|error| ZoningPayloadError::Invalid { item_id, error })?;
    if scene.material.absorption_unit != AbsorptionUnit::PerMm {
        return Err(ZoningPayloadError::MaterialNotPerMm { item_id });
    }
    if !scene.fluorescence.is_empty() {
        return Err(ZoningPayloadError::FluorescentScene { item_id });
    }
    Ok(())
}

impl ZoningPayload {
    /// The payload for a single-scene request, or `None` when the scene's material has no
    /// zones (nothing to send).
    #[must_use]
    pub fn for_scene(request_id: u32, scene: &SceneState) -> Option<Self> {
        scene.material.zoning.as_ref().map(|zoning| Self {
            request_id,
            materials: vec![ZonedMaterialEntry {
                item_id: None,
                zoning: zoning.clone(),
            }],
        })
    }

    /// The payload for a batch, one entry per zoned item in item order, or `None` when no
    /// item is zoned.
    #[must_use]
    pub fn for_batch(request_id: u32, items: &[super::BatchItem]) -> Option<Self> {
        let mut materials: Vec<ZonedMaterialEntry> = items
            .iter()
            .filter_map(|item| {
                item.scene
                    .material
                    .zoning
                    .as_ref()
                    .map(|zoning| ZonedMaterialEntry {
                        item_id: Some(item.item_id),
                        zoning: zoning.clone(),
                    })
            })
            .collect();
        if materials.is_empty() {
            return None;
        }
        materials.sort_by_key(|entry| entry.item_id);
        Some(Self {
            request_id,
            materials,
        })
    }

    /// Checks the payload's own shape (count, duplicates), without looking at any scene.
    ///
    /// # Errors
    ///
    /// [`ZoningPayloadError::Empty`], [`ZoningPayloadError::TooManyEntries`] or
    /// [`ZoningPayloadError::DuplicateTarget`].
    pub fn check_shape(&self) -> Result<(), ZoningPayloadError> {
        if self.materials.is_empty() {
            return Err(ZoningPayloadError::Empty);
        }
        if self.materials.len() > MAX_ZONED_ENTRIES {
            return Err(ZoningPayloadError::TooManyEntries {
                count: self.materials.len(),
            });
        }
        for (index, entry) in self.materials.iter().enumerate() {
            if self.materials[..index]
                .iter()
                .any(|earlier| earlier.item_id == entry.item_id)
            {
                return Err(ZoningPayloadError::DuplicateTarget {
                    item_id: entry.item_id,
                });
            }
        }
        Ok(())
    }

    /// Installs the zones on a single-scene request's `scene` (after validating them).
    ///
    /// # Errors
    ///
    /// Any [`ZoningPayloadError`]; `scene` is left untouched on error.
    pub fn attach_to_scene(&self, scene: &mut SceneState) -> Result<(), ZoningPayloadError> {
        self.check_shape()?;
        let [entry] = self.materials.as_slice() else {
            return Err(ZoningPayloadError::WrongShape);
        };
        if entry.item_id.is_some() {
            return Err(ZoningPayloadError::WrongShape);
        }
        check(scene, &entry.zoning, None)?;
        scene.material.zoning = Some(entry.zoning.clone());
        Ok(())
    }

    /// Installs the zones on the named items of a batch (after validating every entry).
    ///
    /// # Errors
    ///
    /// Any [`ZoningPayloadError`]; `items` is left untouched on error.
    pub fn attach_to_batch(
        &self,
        items: &mut [super::BatchItem],
    ) -> Result<(), ZoningPayloadError> {
        self.check_shape()?;
        // Validate everything first so a failure leaves the batch as it was.
        let mut targets: Vec<(usize, &ZonedAbsorption)> = Vec::with_capacity(self.materials.len());
        for entry in &self.materials {
            let Some(item_id) = entry.item_id else {
                return Err(ZoningPayloadError::WrongShape);
            };
            let Some(index) = items.iter().position(|item| item.item_id == item_id) else {
                return Err(ZoningPayloadError::UnknownItem { item_id });
            };
            check(&items[index].scene, &entry.zoning, Some(item_id))?;
            targets.push((index, &entry.zoning));
        }
        for (index, zoning) in targets {
            items[index].scene.material.zoning = Some(zoning.clone());
        }
        Ok(())
    }
}

/// Writes `payload` as a `ClientMessage::ZoningPayload` frame.
///
/// Only send it to a peer that advertised the zoning capability ([`Welcome::zoning`]).
///
/// # Errors
///
/// [`NetError`] if encoding or writing fails.
pub fn write_zoning_payload<W: Write>(
    writer: &mut W,
    payload: &ZoningPayload,
) -> Result<(), NetError> {
    super::write_message(
        writer,
        &super::ClientMessage::ZoningPayload(Box::new(payload.clone())),
    )
}

/// Writes `hello` as one frame, appending [`ZONING_TAIL`] when `hello.zoning` is set.
///
/// With `hello.zoning == false` the frame is byte-identical to
/// [`super::write_message`]'s.
///
/// # Errors
///
/// [`NetError`] if encoding or writing fails.
pub fn write_hello_message<W: Write>(writer: &mut W, hello: &Hello) -> Result<(), NetError> {
    let mut bytes = postcard::to_allocvec(hello)?;
    if hello.zoning {
        bytes.extend_from_slice(&ZONING_TAIL);
    }
    framing::write_frame(writer, &bytes)?;
    Ok(())
}

/// Writes `welcome` as one frame, appending [`ZONING_TAIL`] when `welcome.zoning` is set.
///
/// A worker must set `welcome.zoning` only if the `HELLO` it answers carried the marker
/// (`Hello::zoning`) -- see the module doc comment.
///
/// # Errors
///
/// [`NetError`] if encoding or writing fails.
pub fn write_welcome_message<W: Write>(writer: &mut W, welcome: &Welcome) -> Result<(), NetError> {
    let mut bytes = postcard::to_allocvec(welcome)?;
    if welcome.zoning {
        bytes.extend_from_slice(&ZONING_TAIL);
    }
    framing::write_frame(writer, &bytes)?;
    Ok(())
}

/// Decodes a `HELLO` frame body, setting [`Hello::zoning`] iff the bytes after the struct are
/// exactly [`ZONING_TAIL`].
///
/// Any other trailing bytes are ignored, as a default build ignores all of them.
///
/// # Errors
///
/// [`postcard::Error`] if the struct does not decode.
pub fn hello_from_bytes(bytes: &[u8]) -> Result<Hello, postcard::Error> {
    let (mut hello, rest) = postcard::take_from_bytes::<Hello>(bytes)?;
    hello.zoning = rest == ZONING_TAIL;
    Ok(hello)
}

/// Splits the capability marker off a `WELCOME` reply frame.
///
/// When `raw` decodes as a [`Welcome`] followed by exactly [`ZONING_TAIL`], returns the frame
/// without the marker and `true`; otherwise returns `raw` unchanged and `false`. The client
/// handshake then runs its usual "decode, remainder must be empty" check on the result, so
/// every OTHER trailing byte is still refused, and sets [`Welcome::zoning`] from the flag.
#[must_use]
pub fn split_welcome_tail(mut raw: Vec<u8>) -> (Vec<u8>, bool) {
    let has_tail = matches!(
        postcard::take_from_bytes::<Welcome>(&raw),
        Ok((_, rest)) if rest == ZONING_TAIL
    );
    if has_tail {
        raw.truncate(raw.len() - ZONING_TAIL.len());
    }
    (raw, has_tail)
}

#[cfg(test)]
mod tests {
    use super::{
        super::{
            BatchItem, ClientMessage, PROTOCOL_VERSION, PayloadEncoding, read_message,
            write_message,
        },
        *,
    };
    use indicatrix::{
        geometry::cuts::StandardGemCuts,
        optics::{
            absorption::AbsorptionTensor,
            materials::GemMaterial,
            raytracer::LightingPreset,
            zoning::{Zone, ZoneAbsorption, ZoneShape},
        },
    };

    fn zoned() -> ZonedAbsorption {
        let mut zoned =
            ZonedAbsorption::new(ZoneAbsorption::per_mm(AbsorptionTensor::isotropic(vec![])));
        zoned.zones.push(Zone {
            shape: ZoneShape::HalfSpace {
                normal: glam::DVec3::X,
                offset: 0.5,
            },
            absorption: ZoneAbsorption::per_mm(AbsorptionTensor::isotropic(vec![])),
        });
        zoned
    }

    fn scene(zoning: Option<ZonedAbsorption>) -> SceneState {
        let mut material = GemMaterial::diamond();
        if let Some(zoning) = zoning {
            material = material.with_zoning(zoning);
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
            environment: crate::scene::SceneEnvironment::Studio,
            surface_glare: 1.0,
            tools: Vec::new(),
            fluorescence: indicatrix::optics::fluorescence::Fluorescence::default(),
            head_shadow_deg: 16.0,
        }
    }

    fn item(item_id: u32, zoning: Option<ZonedAbsorption>) -> BatchItem {
        BatchItem {
            item_id,
            scene: scene(zoning),
            first_sample: 0,
            samples: 4,
            width: 8,
            height: 8,
        }
    }

    /// The scene a zoned material sends over the wire is byte-identical to the same
    /// material's scene without the zones field: `zoning` is `serde(skip)`.
    #[test]
    fn a_zoned_scene_serialises_like_its_unzoned_twin() {
        let zoned_scene = scene(Some(zoned()));
        let mut stripped = zoned_scene.clone();
        stripped.material.zoning = None;
        assert_eq!(
            postcard::to_allocvec(&zoned_scene).unwrap(),
            postcard::to_allocvec(&stripped).unwrap()
        );
    }

    #[test]
    fn zoning_payload_round_trips_for_a_scene_and_for_a_batch() {
        let single = ZoningPayload::for_scene(5, &scene(Some(zoned()))).unwrap();
        let mut buf = Vec::new();
        write_zoning_payload(&mut buf, &single).unwrap();
        let decoded: ClientMessage = read_message(&mut std::io::Cursor::new(buf)).unwrap();
        assert_eq!(decoded, ClientMessage::ZoningPayload(Box::new(single)));

        let items = vec![
            item(3, Some(zoned())),
            item(1, None),
            item(2, Some(zoned())),
        ];
        let batch = ZoningPayload::for_batch(9, &items).unwrap();
        assert_eq!(
            batch
                .materials
                .iter()
                .map(|m| m.item_id)
                .collect::<Vec<_>>(),
            vec![Some(2), Some(3)],
            "zoned items only, ascending"
        );
        let mut buf = Vec::new();
        write_message(
            &mut buf,
            &ClientMessage::ZoningPayload(Box::new(batch.clone())),
        )
        .unwrap();
        let decoded: ClientMessage = read_message(&mut std::io::Cursor::new(buf)).unwrap();
        assert_eq!(decoded, ClientMessage::ZoningPayload(Box::new(batch)));

        assert!(ZoningPayload::for_scene(1, &scene(None)).is_none());
        assert!(ZoningPayload::for_batch(1, &[item(1, None)]).is_none());
    }

    /// Appended at the END of `ClientMessage`: every older variant keeps its index.
    #[test]
    fn the_variant_is_appended_at_index_nine_and_older_indices_do_not_move() {
        let payload = ZoningPayload::for_scene(1, &scene(Some(zoned()))).unwrap();
        let tagged =
            postcard::to_allocvec(&ClientMessage::ZoningPayload(Box::new(payload))).unwrap();
        assert_eq!(tagged[0], 9);
        assert_eq!(
            postcard::to_allocvec(&ClientMessage::Cancel(super::super::Cancel {
                request_id: 1
            }))
            .unwrap()[0],
            0
        );
        assert_eq!(
            postcard::to_allocvec(&ClientMessage::Ping { nonce: 1 }).unwrap()[0],
            4
        );
        assert_eq!(PROTOCOL_VERSION, 24, "zoning needs no protocol bump");
    }

    #[test]
    fn attach_to_scene_installs_the_zones_and_a_stripped_wire_scene_renders_the_same() {
        let local = scene(Some(zoned()));
        // What the worker receives: the scene as decoded (zones skipped) ...
        let bytes = postcard::to_allocvec(&local).unwrap();
        let mut received: SceneState = postcard::from_bytes(&bytes).unwrap();
        assert!(received.material.zoning.is_none());
        // ... plus the payload.
        let payload = ZoningPayload::for_scene(4, &local).unwrap();
        payload.attach_to_scene(&mut received).unwrap();
        assert_eq!(received.material, local.material);
        assert_eq!(received, local);
    }

    #[test]
    fn attach_to_batch_installs_only_the_named_items() {
        let items_local = vec![
            item(1, Some(zoned())),
            item(2, None),
            item(3, Some(zoned())),
        ];
        let payload = ZoningPayload::for_batch(2, &items_local).unwrap();
        let mut received: Vec<BatchItem> = items_local
            .iter()
            .map(|i| {
                let mut stripped = i.clone();
                stripped.scene.material.zoning = None;
                stripped
            })
            .collect();
        payload.attach_to_batch(&mut received).unwrap();
        assert_eq!(received, items_local);
    }

    #[test]
    fn a_bad_payload_is_refused_and_leaves_the_scene_untouched() {
        let mut target = scene(None);
        // Not per-mm.
        target.material.absorption_unit = AbsorptionUnit::ModelUnit;
        let payload = ZoningPayload::for_scene(1, &scene(Some(zoned()))).unwrap();
        assert_eq!(
            payload.attach_to_scene(&mut target),
            Err(ZoningPayloadError::MaterialNotPerMm { item_id: None })
        );
        assert!(target.material.zoning.is_none());

        // Invalid zones: five zones.
        let mut too_many = zoned();
        for _ in 0..5 {
            let zone = too_many.zones[0].clone();
            too_many.zones.push(zone);
        }
        let mut target = scene(Some(zoned()));
        target.material.zoning = None;
        let bad = ZoningPayload {
            request_id: 1,
            materials: vec![ZonedMaterialEntry {
                item_id: None,
                zoning: too_many,
            }],
        };
        assert!(matches!(
            bad.attach_to_scene(&mut target),
            Err(ZoningPayloadError::Invalid { item_id: None, .. })
        ));
        assert!(target.material.zoning.is_none());

        // Wrong shape and unknown item.
        let batch_payload = ZoningPayload::for_batch(1, &[item(7, Some(zoned()))]).unwrap();
        assert_eq!(
            batch_payload.attach_to_scene(&mut scene(None)),
            Err(ZoningPayloadError::WrongShape)
        );
        let mut items = vec![item(1, None)];
        assert_eq!(
            batch_payload.attach_to_batch(&mut items),
            Err(ZoningPayloadError::UnknownItem { item_id: 7 })
        );
        assert_eq!(
            ZoningPayload {
                request_id: 1,
                materials: Vec::new()
            }
            .check_shape(),
            Err(ZoningPayloadError::Empty)
        );
    }

    fn hello_with(zoning: bool) -> Hello {
        let mut hello = Hello::viewer(PROTOCOL_VERSION, [1; 8], [2; 8]);
        hello.accept_encodings = vec![PayloadEncoding::Raw];
        hello.zoning = zoning;
        hello
    }

    fn welcome_with(zoning: bool) -> Welcome {
        Welcome {
            protocol_version: PROTOCOL_VERSION,
            build_hash: [3; 8],
            source_hash: [4; 8],
            render: None,
            library: true,
            tilt_curves: false,
            registration: None,
            payload_encoding: PayloadEncoding::Raw,
            zoning,
        }
    }

    /// The default-build bytes of a handshake message equal the zoning-build bytes when the
    /// capability is off, and equal them plus the marker when it is on; the marker is the only
    /// difference, and `write_message` (what a default build uses) never writes it.
    #[test]
    fn capability_encoding_is_the_old_bytes_plus_the_marker() {
        for zoning in [false, true] {
            let hello = hello_with(zoning);
            let old = postcard::to_allocvec(&hello).unwrap();
            let mut plain = Vec::new();
            write_message(&mut plain, &hello).unwrap();
            let mut ext = Vec::new();
            write_hello_message(&mut ext, &hello).unwrap();
            let (_, plain_body) = plain.split_at(4);
            let (_, ext_body) = ext.split_at(4);
            assert_eq!(
                plain_body,
                old.as_slice(),
                "write_message never adds the marker"
            );
            let mut expected = old;
            if zoning {
                expected.extend_from_slice(&ZONING_TAIL);
            }
            assert_eq!(ext_body, expected.as_slice());
        }
    }

    #[test]
    fn the_marker_round_trips_and_a_default_style_decode_ignores_it() {
        let mut ext = Vec::new();
        write_hello_message(&mut ext, &hello_with(true)).unwrap();
        let body = &ext[4..];
        // A zoning reader sees the bit ...
        assert!(hello_from_bytes(body).unwrap().zoning);
        // ... a default build's `from_bytes` decodes the same struct and ignores the tail.
        let default_style: Hello = postcard::from_bytes(body).unwrap();
        assert_eq!(default_style, hello_with(false));
        // No marker, no bit.
        let mut plain = Vec::new();
        write_hello_message(&mut plain, &hello_with(false)).unwrap();
        assert!(!hello_from_bytes(&plain[4..]).unwrap().zoning);
    }

    #[test]
    fn a_welcome_marker_is_split_off_and_nothing_else_is() {
        let mut with = Vec::new();
        write_welcome_message(&mut with, &welcome_with(true)).unwrap();
        let (stripped, tail) = split_welcome_tail(with[4..].to_vec());
        assert!(tail);
        // What is left is exactly the pre-zoning encoding of the same struct.
        assert_eq!(
            stripped,
            postcard::to_allocvec(&welcome_with(false)).unwrap()
        );

        let mut without = Vec::new();
        write_welcome_message(&mut without, &welcome_with(false)).unwrap();
        let (unchanged, tail) = split_welcome_tail(without[4..].to_vec());
        assert!(!tail);
        assert_eq!(unchanged, without[4..].to_vec());

        // Other trailing bytes are left in place, so the client's empty-remainder check still
        // refuses them like the default handshake does.
        let mut garbage = without[4..].to_vec();
        garbage.push(0xEE);
        let (kept, tail) = split_welcome_tail(garbage.clone());
        assert!(!tail);
        assert_eq!(kept, garbage);
        let mut half_marker = without[4..].to_vec();
        half_marker.extend_from_slice(&ZONING_TAIL[..2]);
        assert!(!split_welcome_tail(half_marker).1);
    }
}
