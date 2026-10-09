//! The wire messages, and encode/decode for each over a length-prefixed
//! [`crate::framing`] stream.
//!
//! ```text
//! -> HELLO    { protocol_version, build_hash, source_hash, role, capability, accept_encodings }
//! <- WELCOME  { protocol_version, build_hash, source_hash, render: Option<RenderCapability>, library, tilt_curves,
//!               registration: Option<WorkerRegistration>, payload_encoding }
//! -> <ClientMessage>  Cancel | Library(LibraryRequest) | RenderRequest | TiltCurvesRequest
//!                     | Ping { nonce } | FinalImageRequest | Asset { content_hash, len } + payload
//!                     | Contribution + payload                        (post-handshake, tagged)
//!                     | ZoningPayload (`zoning` feature only; no protocol bump, see `zoning`)
//! <- <StreamEvent>    Frame | Preview | Progress | Done | Error
//!                     | Pong { nonce } | DisplayFrame | FinalImage | CapabilityChanged { render }
//!                     | NeedAsset { content_hash }                      (tagged)
//! <- <TiltCurvesResponse>  Curves | Cancelled | Error   (TILT_CURVES's single reply, not a stream)
//! ```
//!
//! Split across submodules by topic:
//!
//! - [`hello`]: handshake messages, `HELLO`/`WELCOME`, peer roles.
//! - [`asset`]: content-addressed assets (v14): `NEED_ASSET`/`ASSET`, the
//!   SHA-256 [`content_hash`] and the bounded payload read.
//! - [`encoding`]: payload-encoding negotiation (v14).
//! - [`adaptive`]: per-frame encoding choice that follows the measured link bandwidth;
//!   needs no wire change (headers name each frame's encoding).
//! - [`render`]: `RENDER`'s request shape (`render` feature only).
//! - [`final_image`]: `FINAL_IMAGE_REQUEST`'s shape and reply semantics (`render` feature
//!   only, v14).
//! - [`contribution`]: `-> CONTRIBUTION`, the viewer's own share of a final-picture
//!   export (`render` feature only, v16).
//! - `batch`: `-> BATCH_RENDER_REQUEST`, several finished pictures per request on one
//!   persistent connection (`render` feature only, v24).
//! - [`tilt`]: `TILT_CURVES`'s request/response shapes (`render` feature only).
//! - [`stream`]: everything else post-handshake -- [`stream::ClientMessage`],
//!   [`stream::StreamEvent`], and [`stream::ErrorMsg`].
//! - [`error_codes`]: the `ErrorMsg::code` vocabulary.
//! - [`codec`]: the generic framed codec ([`codec::NetError`],
//!   [`codec::write_message`]/[`codec::read_message`]) every one of the above builds on.
//!
//! Every item is re-exported here at its original flat `messages::` path.

pub mod adaptive;
/// Content-addressed assets (v14) -- see this module's own "Modules" doc section.
pub mod asset;
/// Batched finished-picture requests (v24) -- see this module's own doc comment.
#[cfg(feature = "render")]
pub mod batch;
mod codec;
#[cfg(feature = "render")]
mod contribution;
mod encoding;
mod encoding_matrix;
/// The `ErrorMsg::code` vocabulary -- see this module's own "Modules" doc section.
pub mod error_codes;
#[cfg(feature = "render")]
mod final_image;
mod hello;
#[cfg(feature = "render")]
mod render;
mod stream;
#[cfg(feature = "render")]
mod tilt;
/// The zoning wire extension (`zoning` feature only; no protocol bump).
///
/// A capability marker after `HELLO`/`WELCOME` and the separate `ZoningPayload` message -- see
/// this module's own doc comment.
#[cfg(feature = "zoning")]
pub mod zoning;

#[cfg(feature = "render")]
pub use asset::write_asset_message;
pub use asset::{
    AssetError, AssetHeader, ContentHash, MAX_ASSET_LEN, content_hash, discard_asset_payload,
    hash_hex, read_asset_payload,
};
#[cfg(feature = "render")]
pub use batch::{
    BatchItem, BatchRenderRequest, BatchReply, MAX_BATCH_ITEMS, MAX_BATCHES_IN_FLIGHT,
};
pub use codec::{
    NetError, decode_control_frame, read_control_message, read_message, read_message_bounded,
    write_message,
};
#[cfg(feature = "render")]
pub use contribution::{
    ContributionError, ContributionHeader, ExpectedContribution, discard_contribution_payload,
    read_contribution_payload, write_contribution_message, write_contribution_message_with_link,
};
pub use encoding::{
    DEFAULT_SERVER_PREFERENCE, DisplayEncoding, LOOPBACK_SERVER_PREFERENCE, PayloadEncoding,
    negotiate,
};
#[cfg(feature = "render")]
pub use final_image::{FinalImageRequest, FinalOutput, WireColorSpace};
pub use hello::{Backend, Hello, PeerRole, RenderCapability, Welcome, WorkerRegistration};
#[cfg(feature = "render")]
pub use render::{PreviewConfig, RenderRequest, RequestIntent, StreamConfig, TransferMode};
pub use stream::{
    BatchDone, BatchItemDoneHeader, BatchItemFailed, BatchItemProgress, Cancel, ClientMessage,
    DisplayFrameHeader, Done, ErrorMsg, FinalImageHeader, FrameHeader, PreviewHeader, Progress,
    Stats, StreamEvent, read_frame_message, read_preview_message, read_stream_event,
    write_frame_message, write_preview_message, write_stream_event,
};
#[cfg(feature = "render")]
pub use tilt::{
    AxisTiltCurves, TILT_CURVE_AXIS_COUNT, TILT_CURVE_POINTS_PER_AXIS, TiltCurvesRequest,
    TiltCurvesResponse, TiltCurvesResult,
};
#[cfg(feature = "zoning")]
pub use zoning::{
    MAX_ZONED_ENTRIES, ZONING_TAIL, ZonedMaterialEntry, ZoningPayload, ZoningPayloadError,
    hello_from_bytes, split_welcome_tail, write_hello_message, write_welcome_message,
    write_zoning_payload,
};

/// The wire protocol version this build of `indicatrix-net` speaks.
///
/// Bumped only for changes to the MESSAGE SHAPES in this module. A change to `indicatrix`'s
/// physics does not touch the wire format and is caught instead by the two-level identity
/// check in [`crate::handshake`] (`build_hash`, and -- when both sides can establish it --
/// `source_hash`).
///
/// This is pre-release software: every peer runs its own private build, so there is no
/// deployed fleet to stay wire-compatible with, and this crate carries no compatibility
/// shim or version-conditional decode path -- [`crate::handshake::verify_compatible`]
/// simply refuses to pair peers speaking different versions. The bump's job is making
/// that refusal a clear, diagnosable handshake failure instead of a peer silently
/// mis-decoding garbage.
///
/// Note: [`StreamEvent::Progress`] doubles as the stream's liveness heartbeat, not just
/// real sample progress -- a peer must not assume otherwise.
///
/// # When you MUST bump this
///
/// The wire codec is [`postcard`], which is **not self-describing**:
///
/// - **Enums are encoded by declaration-order index, not name.** An older peer sees an
///   unrecognized index on an appended variant and fails outright -- it cannot skip it.
/// - **Structs are encoded as fields in declaration order, no names, no length prefix.**
///   Appending a field is worse: an older peer silently MISALIGNS every field after it.
///   Applies transitively to anything a wire struct embeds (e.g.
///   [`crate::scene::SceneState`]'s `GemMaterial`, inside [`RenderRequest`]).
///
/// So: append a variant to any wire enum, or a field to any wire struct (or anything it
/// embeds), and bump this -- turning silent corruption into an up-front, diagnosable
/// version-mismatch log line.
///
/// **`#[serde(default)]` does not help here**: it matters only for self-describing
/// formats (e.g. `indicatrix-worker`'s local `scene.json`), not postcard's fixed-layout
/// encoding.
///
/// 9: three lighting models appended to `LightingPreset`.
/// 10: `LightingPreset` 4-6 redefined (ISO hemisphere with head shadow, light tent,
///     daylight sky). Same encoding, but a peer on 9 shades those scenes differently, so
///     tiles from mixed versions would not match.
/// 11: `SceneState::backdrop` appended (the card the camera sees behind the stone).
/// 12: `Hello`/`Welcome` gained `source_hash`: `build_hash` alone is a
///     hash of `indicatrix`'s crate VERSION, a release-process promise nothing enforces,
///     so a viewer and worker whose physics differ but whose version didn't get bumped
///     could pair silently. `source_hash` is a content hash of the actual `indicatrix`
///     source tree; see [`crate::handshake`] for how the two fields are checked together.
/// 13: `LibraryRequest::Search` gained `order` (`SortOrderWire`) and `tag_filter`:
///     without them, the library panel's sort selector and tag chip are silently
///     ignored in remote mode, even though
///     `indicatrix-worker` could already honour both via
///     `indicatrix_vault::db::sqlite::Database::search_diagrams_display`.
/// 14: coordinator mode and lossless payload compression, one bump for all of it:
///     `Hello` gained `role`/`capability`/`accept_encodings`; `Welcome` gained
///     `registration`/`payload_encoding`; `Backend::Coordinator`; `RenderRequest.intent`;
///     `TransferMode::DisplayOnly`; `FrameHeader`/`PreviewHeader` gained
///     `encoding`/`raw_len`; `ClientMessage::{Ping, FinalImageRequest}`;
///     `StreamEvent::{Pong, DisplayFrame, FinalImage, CapabilityChanged}`; error codes
///     `UNSUPPORTED_REQUEST`, `ALL_WORKERS_LOST`, `ROLE_REFUSED`. The first three
///     `Hello`/`Welcome` fields are unchanged, so either side can still read the other's
///     version and refuse with a message naming both (`crate::handshake::read_hello`).
///     HDR environments over the protocol were added to the same unreleased v14:
///     `SceneState::environment` (`SceneEnvironment::{Studio, Hdr}`),
///     `RenderCapability::hdr`, `ClientMessage::Asset`, `StreamEvent::NeedAsset` and the
///     error code `ASSET_FAILED` -- see [`asset`].
/// 15: `ErrorMsg` gained `request_id` (`Option<u32>`): a worker's `StreamEvent::Error`
///     now names the request it concerns (when it concerns exactly one), so
///     [`crate::client::accumulate::Accumulator::apply`] can drop a late error for an
///     epoch the client has already moved on from instead of failing whatever request
///     happens to be current -- see [`ErrorMsg`]'s own doc comment. The design-library
///     protocol's `DesignRecord` also grew new fields under this same bump (a separate
///     lane's change, sharing this version rather than bumping twice).
/// 16: `FinalImageRequest.viewer_samples`; `ClientMessage::Contribution` (index 7) +
///     raw payload; `Stats.reclaimed_samples` -- the viewer renders the tail of a
///     final-picture range itself and uploads float XYZ; see `messages::contribution`.
/// 17: `library::DesignSummary::design_version` appended (the design's revision token,
///     so a mirror detects an edit that leaves the search summary unchanged), and
///     `library::DesignRecord::version` redefined to carry that same token instead of a
///     content hash.
/// 18: `SceneState::surface_glare` appended (the cross-polarised scale of the first
///     surface reflection of the analytic lighting presets).
/// 19: concave facets. `SceneState::tools` appended (`Vec<ToolPrimitive>`, the convex
///     tool volumes subtracted from the plane polyhedron; always present on the wire,
///     empty for a convex stone); `library::DesignSummary::concave_tiers` and
///     `library::AngleSettingWire::tool_line` appended;
///     `LibraryRequest::FetchDesignNative` and `LibraryResponse::{DesignNative,
///     DesignNativeNotAvailable}` appended. `DesignSummary::version` now also hashes
///     `concave_tiers`, so every design's stamp changes once and each mirror re-syncs.
/// 20: fluorescence and UV lamps. `LightingPreset::{UvLamp365, UvLamp395}` appended (enum
///     indices 7 and 8) and `SceneState::fluorescence` appended
///     (`indicatrix::optics::fluorescence::Fluorescence`, the material's emitters; always
///     present on the wire, an empty `Vec` of emitters for a non-fluorescent material).
///     A scene with fluorescence or a UV lamp is traced on the CPU only.
/// 21: `SceneState::head_shadow_deg` appended (`f32`, the viewer's head-shadow radius of
///     the lit lighting presets; `0.0` off, default `16.0`).
/// 22: lighting wave. `LightingPreset::{DaylightSun, Aset, ShopLights, WindowDaylight,
///     WhiteTray, IlluminantA}` appended (enum indices 9 to 14; `SceneState` carries the
///     enum), and `LightingModel::{Aset, DaylightSun}` (GPU model ids 4 and 5) added.
/// 23: absorption units. `indicatrix::optics::materials::GemMaterial` (inside
///     `SceneState::material`) gained `absorption_unit` (`AbsorptionUnit`, enum index 0
///     `ModelUnit` / 1 `PerMm`, one byte, declared right after `absorption_path_scale`), and
///     the tracer no longer scales the inclusion `scattering_sigma_s` by the path scale (it is
///     per model unit, independent of the stone's size). A peer on 22 would misalign the
///     material and trace scattering scenes with the old rule.
/// 24: batched preview requests. `ClientMessage::BatchRenderRequest` (index 8, `render`-gated)
///     and `StreamEvent::{BatchItemProgress, BatchItemDone, BatchItemFailed, BatchDone}`
///     (indices 10 to 13) appended; see `messages::batch`. `BatchItemDone` carries a raw PNG payload
///     frame. Nothing existing changed shape, but a peer on 23 would fail to decode the new
///     indices outright, so the bump turns that into a handshake refusal.
///
/// A `zoning` build (off by default) appends `ClientMessage::ZoningPayload` (index 9) WITHOUT a
/// bump: it is sent only to a peer whose `WELCOME` carried the zoning capability marker, which a
/// peer sends only in reply to a `HELLO` that carried the marker -- so a default build never
/// sees the message or the marker. See the `zoning` module.
pub const PROTOCOL_VERSION: u16 = 24;

#[cfg(test)]
mod tests {
    #[test]
    /// Pins the constant so a bump is always a deliberate, reviewed edit.
    ///
    /// 21: `SceneState::head_shadow_deg` (see the constant's history).
    /// 22: lighting wave (new `LightingPreset` and `LightingModel` variants).
    /// 23: absorption units (`GemMaterial::absorption_unit`; scattering unscaled).
    /// 24: batched preview requests (`ClientMessage::BatchRenderRequest` and the four
    ///     `StreamEvent::Batch*` events).
    fn protocol_version_matches_constant() {
        assert_eq!(super::PROTOCOL_VERSION, 24);
    }
}
