//! Decodes and caches the two cached preview PNGs (`diagram_previews.preview_front`/
//! `preview_top`) per design for `diagram_list.slint`'s two small per-row thumbnails.
//!
//! This is a synchronous Slint callback backed by a background-decoded cache, not a
//! field on `DiagramItem`: `gui::search::to_diagram_item` (the only place `DiagramItem`
//! rows are built) is owned by another agent concurrently and can't be touched. Instead
//! `diagram_list.slint` calls `root.get_preview_thumbnails(item.id,
//! root.preview_cache_version)` per visible row, and
//! [`setup_preview_thumbnail_callback`] answers purely from `item.id`.
//!
//! The callback runs on the UI thread and must return fast: a cache hit is a plain
//! `HashMap` lookup; a miss marks the entry `Loading`, spawns a background thread to
//! fetch the PNG from SQLite and decode it (`image::load_from_memory`, non-trivial
//! across a list of thousands), and returns a placeholder immediately. When decoding
//! finishes, the background thread stores the result and bumps `preview_cache_version`,
//! which every row's `get_preview_thumbnails` call also reads, forcing Slint to
//! re-evaluate every visible row against the now-populated cache.
//!
//! The cache stores a decoded [`slint::SharedPixelBuffer<Rgba8Pixel>`], not a
//! `slint::Image`, matching this crate's convention for anything crossing from a
//! background thread to the UI thread (see `bridge::export_thread::ExportProgress`).
//! `slint::Image::from_rgba8` is only ever called on the UI thread, from an
//! already-decoded buffer -- a cheap refcounted wrap, no decode work.
//!
//! A row with no cached previews yet (never generated, or still decoding) reports
//! `has_front`/`has_top: false`; `diagram_list.slint` reserves the same fixed-size box
//! either way and shows a placeholder rectangle, so no row shows a broken/stretched
//! image or shifts layout as previews arrive.

use crate::{LibraryModel, MainWindow, PreviewThumbnails};
use indicatrix_vault::db::sqlite::Database;
use slint::{ComponentHandle, Rgba8Pixel, SharedPixelBuffer};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex, PoisonError},
    thread,
};

/// One entry's cached state. `Loading` exists so a second `get_preview_thumbnails` call
/// for the same id, arriving before the first background decode finishes (e.g. the row
/// scrolls out and back into view), doesn't spawn a SECOND redundant decode thread for
/// it.
enum CacheState {
    Loading,
    /// `None` for a view that was never generated, or whose PNG failed to decode --
    /// both mean "show the placeholder for this view".
    Ready {
        front: Option<SharedPixelBuffer<Rgba8Pixel>>,
        top: Option<SharedPixelBuffer<Rgba8Pixel>>,
    },
}

type Cache = Arc<Mutex<HashMap<i64, CacheState>>>;

/// Registers `MainWindow::get_preview_thumbnails` -- see this module's doc comment.
/// Owns its cache for the life of the window; no eviction, deliberately: even a full
/// catalogue's worth of 160x160 RGBA8 buffers (two per design) is well under 200MB, and
/// a diagram list is browsed, not scrolled through the millions of rows eviction would
/// matter for.
pub fn setup_preview_thumbnail_callback(ui: &MainWindow, db: &Arc<Mutex<Database>>) {
    let cache: Cache = Arc::new(Mutex::new(HashMap::new()));
    let db = Arc::clone(db);
    let ui_weak = ui.as_weak();

    // `_cache_version` exists only so the Slint call expression reads
    // `preview_cache_version`, driving re-evaluation on a cache update; unused here.
    ui.global::<LibraryModel>()
        .on_get_preview_thumbnails(move |id: i32, _cache_version: i32| {
            let entry_id = i64::from(id);
            {
                let guard = cache.lock().unwrap_or_else(PoisonError::into_inner);
                match guard.get(&entry_id) {
                    Some(CacheState::Ready { front, top }) => {
                        return to_slint_thumbnails(front.as_ref(), top.as_ref());
                    }
                    Some(CacheState::Loading) => return PreviewThumbnails::default(),
                    None => {}
                }
            }
            // Cache miss: mark loading, kick off the decode in the background, and return
            // a placeholder for this call.
            cache
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .insert(entry_id, CacheState::Loading);

            let db = Arc::clone(&db);
            let cache = Arc::clone(&cache);
            let ui_weak = ui_weak.clone();
            thread::spawn(move || {
                let images = {
                    let guard = db.lock().unwrap_or_else(PoisonError::into_inner);
                    guard.get_preview_images(entry_id).unwrap_or_default()
                };
                let front = images.front.as_deref().and_then(decode_to_pixel_buffer);
                let top = images.top.as_deref().and_then(decode_to_pixel_buffer);

                cache
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .insert(entry_id, CacheState::Ready { front, top });

                let _ = ui_weak.upgrade_in_event_loop(|ui| {
                    // Forces every visible row's thumbnail lookup to re-evaluate.
                    ui.global::<LibraryModel>().set_preview_cache_version(
                        ui.global::<LibraryModel>().get_preview_cache_version() + 1,
                    );
                });
            });

            PreviewThumbnails::default()
        });
}

/// Decodes `png` into an RGBA8 `SharedPixelBuffer`, or `None` on any decode failure --
/// treated by [`setup_preview_thumbnail_callback`] exactly like "never generated".
fn decode_to_pixel_buffer(png: &[u8]) -> Option<SharedPixelBuffer<Rgba8Pixel>> {
    let decoded = image::load_from_memory(png).ok()?.to_rgba8();
    let (width, height) = decoded.dimensions();
    let mut buffer = SharedPixelBuffer::<Rgba8Pixel>::new(width, height);
    let dst: &mut [Rgba8Pixel] = buffer.make_mut_slice();
    let src: &[Rgba8Pixel] = bytemuck::cast_slice(decoded.as_raw());
    dst.copy_from_slice(src);
    Some(buffer)
}

fn to_slint_thumbnails(
    front: Option<&SharedPixelBuffer<Rgba8Pixel>>,
    top: Option<&SharedPixelBuffer<Rgba8Pixel>>,
) -> PreviewThumbnails {
    PreviewThumbnails {
        front: front.map_or_else(slint::Image::default, |buf| {
            slint::Image::from_rgba8(buf.clone())
        }),
        top: top.map_or_else(slint::Image::default, |buf| {
            slint::Image::from_rgba8(buf.clone())
        }),
        has_front: front.is_some(),
        has_top: top.is_some(),
    }
}
