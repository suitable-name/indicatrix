//! `-> BATCH_RENDER_REQUEST` (v24): several finished pictures in one request.
//!
//! The catalogue-preview batch used to open one fresh connection per picture and ask for
//! the full float radiance back; a GPU worker spent most of its time idle between
//! pictures. A batch request names up to [`MAX_BATCH_ITEMS`] pictures at once, each with
//! its own scene and sample range, and the worker answers with FINISHED, tone-mapped PNGs
//! as each is ready:
//!
//! ```text
//! -> BatchRenderRequest { request_id, reply, items: [BatchItem { item_id, scene, first_sample,
//!                                                              samples, width, height }, ..] }
//! <- BATCH_ITEM_PROGRESS { request_id, item_id, samples_done }   -- per item, as it is traced
//! <- PROGRESS            { request_id, samples_done }            -- heartbeat, at least every 2 s
//! <- BATCH_ITEM_DONE     { request_id, item_id, samples_done, payload_len } + raw PNG bytes
//! <- BATCH_ITEM_FAILED   { request_id, item_id, reason }         -- that item only
//! <- BATCH_DONE          { request_id, cancelled }               -- exactly once, last
//! ```
//!
//! # Persistent connection, two batches in flight
//!
//! A client keeps ONE connection for a whole run and sends the NEXT `BatchRenderRequest`
//! while the current one is still tracing, so the worker always has queued work. A worker
//! accepts at most [`MAX_BATCHES_IN_FLIGHT`] unfinished batches per connection; a third is
//! refused with `VALIDATION_FAILED`. Batches run in the order they arrive.
//!
//! # Replies
//!
//! Every item is answered exactly once, by `BATCH_ITEM_DONE` or `BATCH_ITEM_FAILED`
//! (never both), except on cancellation: a `CANCEL` carrying the batch's `request_id`
//! stops the items not yet finished, which are simply never answered, and the batch ends
//! with `BATCH_DONE { cancelled: true }`. Items answered before the cancel stay valid.
//! A batch the worker cannot serve at all is refused with `StreamEvent::Error` (no
//! `BATCH_DONE`), like any other refused request.
//!
//! The PNG a [`BatchReply::FinalPng`] item carries is byte-identical to what the viewer's
//! own preview path writes from the same float sum: the worker divides by `samples`,
//! tone-maps with `indicatrix::renderer::tonemap::tonemap_to_rgba` at sRGB and encodes
//! with `indicatrix::render_setup::png_encode::encode_preview_png`, the one function
//! both sides call.
//!
//! Only compiled under this crate's `render` feature, like `RenderRequest`.

use crate::scene::SceneState;
use serde::{Deserialize, Serialize};

/// The most items one [`BatchRenderRequest`] may carry. The worker refuses more with
/// `VALIDATION_FAILED`; the viewer's setting is clamped to this.
pub const MAX_BATCH_ITEMS: usize = 32;

/// The most unfinished batches a worker holds per connection (one tracing, one queued).
pub const MAX_BATCHES_IN_FLIGHT: usize = 2;

/// What a [`BatchRenderRequest`] wants back for each item. Variant order is
/// wire-load-bearing; append only.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BatchReply {
    /// A finished, tone-mapped sRGB PNG per item, as `BATCH_ITEM_DONE`'s payload frame.
    /// Previews always use this.
    FinalPng,
}

/// One picture of a [`BatchRenderRequest`]: its scene and the sample range to trace.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BatchItem {
    /// Client-chosen id, unique within its batch, echoed on this item's replies.
    pub item_id: u32,
    /// The fully resolved scene to trace (studio-lit: an HDR environment is refused with
    /// `BATCH_ITEM_FAILED`).
    pub scene: SceneState,
    /// First absolute sample index.
    pub first_sample: u32,
    /// Number of samples; the tone-mapping divisor.
    pub samples: u32,
    /// Output width in pixels; must equal `scene.width`.
    pub width: u32,
    /// Output height in pixels; must equal `scene.height`.
    pub height: u32,
}

impl BatchItem {
    /// Whether the output size matches the scene, the only size a worker accepts.
    #[must_use]
    pub const fn output_matches_scene(&self) -> bool {
        self.width == self.scene.width && self.height == self.scene.height
    }
}

/// `-> BATCH_RENDER_REQUEST`: trace every item and reply with its picture -- see the
/// module doc comment for the reply sequence.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BatchRenderRequest {
    /// Client-chosen epoch id of the BATCH, echoed on every reply. Unique among the
    /// batches in flight on a connection.
    pub request_id: u32,
    /// What each item's reply carries.
    pub reply: BatchReply,
    /// The pictures, answered individually; at most [`MAX_BATCH_ITEMS`].
    pub items: Vec<BatchItem>,
}

#[cfg(test)]
mod tests {
    use super::{
        super::codec::{read_message, write_message},
        *,
    };

    fn scene(width: u32, height: u32) -> SceneState {
        use indicatrix::{
            geometry::cuts::StandardGemCuts,
            optics::{materials::GemMaterial, raytracer::LightingPreset},
        };
        SceneState {
            width,
            height,
            yaw: 0.4,
            pitch: 0.3,
            distance: 3.0,
            light_yaw: 0.85,
            light_pitch: 0.95,
            exposure: 1.0,
            max_bounces: 4,
            lighting_preset: LightingPreset::Daylight,
            material: GemMaterial::diamond(),
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

    fn request(items: u32) -> BatchRenderRequest {
        BatchRenderRequest {
            request_id: 7,
            reply: BatchReply::FinalPng,
            items: (0..items)
                .map(|i| BatchItem {
                    item_id: i,
                    scene: scene(8 + i, 6),
                    first_sample: 64 * i,
                    samples: 16,
                    width: 8 + i,
                    height: 6,
                })
                .collect(),
        }
    }

    #[test]
    fn a_batch_request_round_trips_with_several_items() {
        let request = request(10);
        let mut buf = Vec::new();
        write_message(&mut buf, &request).unwrap();
        let decoded: BatchRenderRequest = read_message(&mut std::io::Cursor::new(buf)).unwrap();
        assert_eq!(decoded, request);
        assert!(decoded.items.iter().all(BatchItem::output_matches_scene));
    }

    #[test]
    fn a_full_batch_of_thirty_two_items_fits_one_control_frame() {
        let mut buf = Vec::new();
        write_message(&mut buf, &request(MAX_BATCH_ITEMS as u32)).unwrap();
        // The frame is `LEN_PREFIX_BYTES` of length followed by the payload; the payload
        // is what `MAX_CONTROL_FRAME_LEN` bounds.
        let payload = buf.len() - crate::framing::LEN_PREFIX_BYTES;
        assert!(
            payload < crate::framing::MAX_CONTROL_FRAME_LEN as usize,
            "32 round-brilliant items must stay under the control-frame cap, got {payload}"
        );
    }

    #[test]
    fn a_size_other_than_the_scenes_is_flagged() {
        let mut item = request(1).items.remove(0);
        item.width += 1;
        assert!(!item.output_matches_scene());
    }

    #[test]
    fn the_reply_mode_discriminant_is_pinned() {
        assert_eq!(postcard::to_allocvec(&BatchReply::FinalPng).unwrap(), [0]);
    }
}
