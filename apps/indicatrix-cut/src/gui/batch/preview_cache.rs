//! Decodes and caches the two cached preview PNGs (`diagram_previews.preview_front`/
//! `preview_top`) per design for `diagram_list.slint`'s two small per-row thumbnails.
//!
//! This is a synchronous Slint callback backed by a background-decoded cache, not a
//! field on `DiagramItem`: `gui::search::to_diagram_item` (the only place `DiagramItem`
//! rows are built) does not decode the cached PNGs, so this resolves them lazily
//! instead. `diagram_list.slint` calls `root.get_preview_thumbnails(item.id,
//! root.preview_cache_version)` per visible row, and
//! [`setup_preview_thumbnail_callback`] answers purely from `item.id`.
//!
//! The callback runs on the UI thread and must return fast: a cache hit is a plain
//! `HashMap` lookup; a miss marks the entry `Loading`, queues the id for ONE decoder
//! thread and returns a placeholder immediately. The decoder takes the newest queued id
//! first (the rows on screen now, not the ones a fast scroll already left behind),
//! fetches its PNGs from SQLite one design at a time, decodes them
//! (`image::load_from_memory`) and stores the result. The queue is bounded: when it
//! overflows, the oldest request is forgotten and simply re-queued by the next
//! evaluation of its row.
//!
//! Finished decodes do not bump `preview_cache_version` one by one. The first completion
//! arms a single UI-thread timer and the bump happens when it fires, so the visible rows
//! re-evaluate against the cache at most about ten times per second however many designs
//! finished in between.
//!
//! The cache keeps at most [`MAX_CACHED_DESIGNS`] decoded designs and evicts the least
//! recently looked-up one beyond that.
//!
//! The preview batch shares the cache through [`PreviewThumbnailCache`]: after it saves a
//! design's previews it calls `invalidate`, which drops the stale entry (a placeholder
//! or the old image) and schedules the same coalesced version bump, so the card
//! re-decodes shortly after.
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
    cell::RefCell,
    collections::{HashMap, VecDeque},
    sync::{
        Arc, Condvar, Mutex, MutexGuard, PoisonError,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Duration,
};
use tracing::warn;

/// How many decoded designs the cache keeps. A 160x160 RGBA8 pair is 204,800 bytes, so
/// this bounds the cache near 123 MB; an unbounded cache over a catalogue of about 3,300
/// designs would hold about 676 MB.
const MAX_CACHED_DESIGNS: usize = 600;

/// How many undecoded requests may wait for the decoder. Comfortably more than the rows
/// on screen, so a fast scroll overflows it only with rows long since scrolled away.
const MAX_QUEUED_REQUESTS: usize = 64;

/// The delay between the first finished decode and the `preview_cache_version` bump that
/// makes the rows pick it up; every decode finishing inside the window shares that bump.
const VERSION_BUMP_INTERVAL: Duration = Duration::from_millis(100);

type Pixels = SharedPixelBuffer<Rgba8Pixel>;

/// One entry's cached state. `Loading` exists so a second `get_preview_thumbnails` call
/// for the same id, arriving before the decode finishes (e.g. the row scrolls out and
/// back into view), does not queue a SECOND redundant decode for it.
enum Entry {
    Loading,
    /// `None` for a view that was never generated, or whose PNG failed to decode --
    /// both mean "show the placeholder for this view".
    Ready {
        front: Option<Pixels>,
        top: Option<Pixels>,
        /// The [`State::tick`] of the latest lookup: the eviction order.
        last_used: u64,
    },
}

/// What one row callback learns about a design.
enum Lookup {
    Ready {
        front: Option<Pixels>,
        top: Option<Pixels>,
    },
    /// Queued or decoding already; the row shows its placeholder.
    Loading,
    /// Not known yet and just queued; the decoder needs waking.
    Requested,
}

/// Everything the UI thread and the decoder thread share, behind one mutex. The decoder
/// holds that mutex only for bookkeeping, never across a database read or a decode.
#[derive(Default)]
struct State {
    entries: HashMap<i64, Entry>,
    /// Ids waiting for the decoder, oldest first.
    queue: VecDeque<i64>,
    /// The id the decoder is working on right now.
    in_flight: Option<i64>,
    /// Set when [`Self::in_flight`] was invalidated mid-decode: the pixels about to be
    /// stored predate the new save and must be discarded.
    in_flight_stale: bool,
    /// Counts lookups; unique per lookup, so eviction never depends on hash order.
    tick: u64,
    /// Set once the window is gone; the decoder thread then returns.
    closed: bool,
}

impl State {
    /// Answers a row's lookup, refreshing the entry's recency on a hit and queueing the
    /// id (dropping the oldest queued request beyond [`MAX_QUEUED_REQUESTS`]) on a miss.
    fn lookup_or_enqueue(&mut self, entry_id: i64) -> Lookup {
        self.tick += 1;
        let tick = self.tick;
        match self.entries.get_mut(&entry_id) {
            Some(Entry::Ready {
                front,
                top,
                last_used,
            }) => {
                *last_used = tick;
                return Lookup::Ready {
                    front: front.clone(),
                    top: top.clone(),
                };
            }
            Some(Entry::Loading) => return Lookup::Loading,
            None => {}
        }
        self.entries.insert(entry_id, Entry::Loading);
        self.queue.push_back(entry_id);
        if self.queue.len() > MAX_QUEUED_REQUESTS
            && let Some(dropped) = self.queue.pop_front()
            && matches!(self.entries.get(&dropped), Some(Entry::Loading))
        {
            self.entries.remove(&dropped);
        }
        Lookup::Requested
    }

    /// Hands the decoder the newest queued id and records it as in flight.
    fn begin_next(&mut self) -> Option<i64> {
        let entry_id = self.queue.pop_back()?;
        self.in_flight = Some(entry_id);
        self.in_flight_stale = false;
        Some(entry_id)
    }

    /// Stores a finished decode, unless the design was invalidated while it ran, then
    /// evicts down to [`MAX_CACHED_DESIGNS`].
    fn store_decoded(&mut self, entry_id: i64, front: Option<Pixels>, top: Option<Pixels>) {
        self.in_flight = None;
        if std::mem::take(&mut self.in_flight_stale) {
            return;
        }
        self.tick += 1;
        let last_used = self.tick;
        self.entries.insert(
            entry_id,
            Entry::Ready {
                front,
                top,
                last_used,
            },
        );
        self.evict_to_capacity();
    }

    /// Removes the least recently looked-up `Ready` entry while more than
    /// [`MAX_CACHED_DESIGNS`] are held. `Loading` markers do not count: the queue bound
    /// already limits them.
    fn evict_to_capacity(&mut self) {
        let ready = self
            .entries
            .values()
            .filter(|entry| matches!(entry, Entry::Ready { .. }))
            .count();
        if ready <= MAX_CACHED_DESIGNS {
            return;
        }
        let oldest = self
            .entries
            .iter()
            .filter_map(|(&id, entry)| match entry {
                Entry::Ready { last_used, .. } => Some((*last_used, id)),
                Entry::Loading => None,
            })
            .min();
        if let Some((_, id)) = oldest {
            self.entries.remove(&id);
        }
    }

    /// Drops everything known about `entry_id` so the next lookup decodes it afresh, and
    /// marks a decode of it that is running right now as stale.
    fn forget(&mut self, entry_id: i64) {
        self.entries.remove(&entry_id);
        self.queue.retain(|&queued| queued != entry_id);
        if self.in_flight == Some(entry_id) {
            self.in_flight_stale = true;
        }
    }
}

/// The state, its decoder wake-up and the pending-bump flag.
struct Shared {
    state: Mutex<State>,
    /// Signalled when a request is queued or the cache is closed.
    wake: Condvar,
    /// `true` from the first finished decode until the timer it armed fires, so a burst
    /// of completions arms one timer.
    bump_pending: AtomicBool,
}

impl Shared {
    fn new() -> Self {
        Self {
            state: Mutex::new(State::default()),
            wake: Condvar::new(),
            bump_pending: AtomicBool::new(false),
        }
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The row callback's entry point; wakes the decoder when it queued a request.
    fn lookup(&self, entry_id: i64) -> Lookup {
        let found = self.lock().lookup_or_enqueue(entry_id);
        if matches!(found, Lookup::Requested) {
            self.wake.notify_one();
        }
        found
    }

    /// Blocks the decoder until a request is queued; `None` once the cache is closed.
    fn next_request(&self) -> Option<i64> {
        let mut state = self
            .wake
            .wait_while(self.lock(), |state| state.queue.is_empty() && !state.closed)
            .unwrap_or_else(PoisonError::into_inner);
        if state.closed {
            return None;
        }
        state.begin_next()
    }

    fn finish(&self, entry_id: i64, front: Option<Pixels>, top: Option<Pixels>) {
        self.lock().store_decoded(entry_id, front, top);
    }

    fn close(&self) {
        self.lock().closed = true;
        self.wake.notify_all();
    }
}

/// Closes the cache when the row callback holding it is dropped, which ends the decoder
/// thread.
struct CloseOnDrop(Arc<Shared>);

impl CloseOnDrop {
    fn shared(&self) -> &Shared {
        &self.0
    }
}

impl Drop for CloseOnDrop {
    fn drop(&mut self) {
        self.0.close();
    }
}

/// The decoded-thumbnail cache `diagram_list.slint` reads; shared with the preview batch
/// so a freshly saved preview replaces the stale entry. Cheap to clone (a shared handle).
#[derive(Clone)]
pub struct PreviewThumbnailCache(Arc<Shared>);

impl PreviewThumbnailCache {
    /// Drops `entry_id`'s cached thumbnails and schedules a `preview_cache_version`
    /// bump, so the visible rows re-query and the next `get_preview_thumbnails` for that
    /// id re-decodes from the database. Safe to call from any thread: the cache is
    /// behind a mutex and the Slint property write is deferred to the event loop.
    pub fn invalidate(&self, ui_weak: &slint::Weak<MainWindow>, entry_id: i64) {
        self.0.lock().forget(entry_id);
        request_version_bump(&self.0, ui_weak);
    }
}

thread_local! {
    /// The timer armed by [`arm_bump_timer`]; kept here so it stays alive until it fires.
    /// UI-thread-only.
    static BUMP_TIMER: RefCell<Option<slint::Timer>> = const { RefCell::new(None) };
}

/// Forces every visible row's thumbnail lookup to re-evaluate. UI thread only.
fn bump_cache_version(ui: &MainWindow) {
    let library = ui.global::<LibraryModel>();
    library.set_preview_cache_version(library.get_preview_cache_version() + 1);
}

/// Asks for a `preview_cache_version` bump within [`VERSION_BUMP_INTERVAL`]. A request
/// made while one is already pending is absorbed by it. Any thread.
fn request_version_bump(shared: &Arc<Shared>, ui_weak: &slint::Weak<MainWindow>) {
    if shared.bump_pending.swap(true, Ordering::AcqRel) {
        return;
    }
    let shared_for_timer = Arc::clone(shared);
    if ui_weak
        .upgrade_in_event_loop(move |ui| arm_bump_timer(&ui, shared_for_timer))
        .is_err()
    {
        // No event loop to bump on; let a later request try again.
        shared.bump_pending.store(false, Ordering::Release);
    }
}

/// Starts the one-shot timer whose firing performs the bump. UI thread only.
fn arm_bump_timer(ui: &MainWindow, shared: Arc<Shared>) {
    let ui_weak = ui.as_weak();
    let timer = slint::Timer::default();
    timer.start(
        slint::TimerMode::SingleShot,
        VERSION_BUMP_INTERVAL,
        move || {
            // Cleared BEFORE bumping: a decode finishing from here on arms a fresh timer
            // instead of being absorbed by one that has already re-evaluated the rows.
            shared.bump_pending.store(false, Ordering::Release);
            if let Some(ui) = ui_weak.upgrade() {
                bump_cache_version(&ui);
            }
        },
    );
    BUMP_TIMER.with(|cell| *cell.borrow_mut() = Some(timer));
}

/// The decoder thread's loop: one design at a time, newest request first.
fn run_decoder(shared: &Arc<Shared>, db: &Mutex<Database>, ui_weak: &slint::Weak<MainWindow>) {
    while let Some(entry_id) = shared.next_request() {
        // The shared database lock is held for this one read only, never across the
        // decode, so the UI thread and other readers interleave between designs.
        let images = db
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get_preview_images(entry_id)
            .unwrap_or_default();
        let front = images.front.as_deref().and_then(decode_to_pixel_buffer);
        let top = images.top.as_deref().and_then(decode_to_pixel_buffer);
        shared.finish(entry_id, front, top);
        request_version_bump(shared, ui_weak);
    }
}

/// Registers `MainWindow::get_preview_thumbnails` -- see this module's doc comment.
/// Owns its cache for the life of the window; the cache keeps at most
/// [`MAX_CACHED_DESIGNS`] decoded designs. Returns the shared handle so the preview
/// batch can invalidate an entry (via [`PreviewThumbnailCache::invalidate`]) after
/// saving new previews.
pub fn setup_preview_thumbnail_callback(
    ui: &MainWindow,
    db: &Arc<Mutex<Database>>,
) -> PreviewThumbnailCache {
    let shared = Arc::new(Shared::new());
    let handle = PreviewThumbnailCache(Arc::clone(&shared));

    let decoder_shared = Arc::clone(&shared);
    let db = Arc::clone(db);
    let ui_weak = ui.as_weak();
    if let Err(e) = thread::Builder::new()
        .name("preview-thumbnail-decoder".into())
        .spawn(move || run_decoder(&decoder_shared, &db, &ui_weak))
    {
        warn!("could not start the preview thumbnail decoder: {e}");
    }

    let lifetime = CloseOnDrop(shared);
    // `_cache_version` exists only so the Slint call expression reads
    // `preview_cache_version`, driving re-evaluation on a cache update; unused here.
    ui.global::<LibraryModel>()
        .on_get_preview_thumbnails(move |id: i32, _cache_version: i32| {
            match lifetime.shared().lookup(i64::from(id)) {
                Lookup::Ready { front, top } => to_slint_thumbnails(front.as_ref(), top.as_ref()),
                Lookup::Loading | Lookup::Requested => PreviewThumbnails::default(),
            }
        });
    handle
}

/// Decodes `png` into an RGBA8 `SharedPixelBuffer`, or `None` on any decode failure --
/// treated by [`setup_preview_thumbnail_callback`] exactly like "never generated".
fn decode_to_pixel_buffer(png: &[u8]) -> Option<Pixels> {
    let decoded = image::load_from_memory(png).ok()?.to_rgba8();
    let (width, height) = decoded.dimensions();
    let mut buffer = SharedPixelBuffer::<Rgba8Pixel>::new(width, height);
    let dst: &mut [Rgba8Pixel] = buffer.make_mut_slice();
    let src: &[Rgba8Pixel] = bytemuck::cast_slice(decoded.as_raw());
    dst.copy_from_slice(src);
    Some(buffer)
}

fn to_slint_thumbnails(front: Option<&Pixels>, top: Option<&Pixels>) -> PreviewThumbnails {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn pixels() -> Pixels {
        SharedPixelBuffer::<Rgba8Pixel>::new(1, 1)
    }

    fn is_ready(state: &State, entry_id: i64) -> bool {
        matches!(state.entries.get(&entry_id), Some(Entry::Ready { .. }))
    }

    #[test]
    fn a_miss_is_queued_once_and_then_reported_loading() {
        let mut state = State::default();
        assert!(matches!(state.lookup_or_enqueue(7), Lookup::Requested));
        assert!(matches!(state.lookup_or_enqueue(7), Lookup::Loading));
        assert_eq!(state.queue, VecDeque::from([7]));
    }

    #[test]
    fn the_decoder_takes_the_newest_request_first() {
        let mut state = State::default();
        for id in [1, 2, 3] {
            state.lookup_or_enqueue(id);
        }
        assert_eq!(state.begin_next(), Some(3));
        assert_eq!(state.begin_next(), Some(2));
        assert_eq!(state.begin_next(), Some(1));
        assert_eq!(state.begin_next(), None);
    }

    #[test]
    fn an_overflowing_queue_forgets_its_oldest_request() {
        let mut state = State::default();
        for id in 0..=MAX_QUEUED_REQUESTS as i64 {
            state.lookup_or_enqueue(id);
        }
        assert_eq!(state.queue.len(), MAX_QUEUED_REQUESTS);
        assert!(!state.entries.contains_key(&0));
        // The forgotten id is requested again by its row's next evaluation.
        assert!(matches!(state.lookup_or_enqueue(0), Lookup::Requested));
    }

    #[test]
    fn the_least_recently_looked_up_design_is_evicted() {
        let mut state = State::default();
        for id in 0..MAX_CACHED_DESIGNS as i64 {
            state.store_decoded(id, Some(pixels()), Some(pixels()));
        }
        // Looking design 0 up makes design 1 the oldest.
        assert!(matches!(state.lookup_or_enqueue(0), Lookup::Ready { .. }));
        state.store_decoded(MAX_CACHED_DESIGNS as i64, Some(pixels()), None);
        assert!(is_ready(&state, 0));
        assert!(!state.entries.contains_key(&1));
        assert!(is_ready(&state, MAX_CACHED_DESIGNS as i64));
        assert_eq!(state.entries.len(), MAX_CACHED_DESIGNS);
    }

    #[test]
    fn a_decode_finishing_after_its_design_was_invalidated_is_discarded() {
        let mut state = State::default();
        state.lookup_or_enqueue(5);
        assert_eq!(state.begin_next(), Some(5));
        state.forget(5);
        state.store_decoded(5, Some(pixels()), Some(pixels()));
        assert!(!state.entries.contains_key(&5));
        // The design decodes afresh on its next lookup.
        assert!(matches!(state.lookup_or_enqueue(5), Lookup::Requested));
        assert_eq!(state.begin_next(), Some(5));
        state.store_decoded(5, Some(pixels()), Some(pixels()));
        assert!(is_ready(&state, 5));
    }

    #[test]
    fn invalidating_a_design_removes_it_from_the_cache_and_the_queue() {
        let mut state = State::default();
        state.store_decoded(1, Some(pixels()), Some(pixels()));
        state.lookup_or_enqueue(2);
        state.forget(1);
        state.forget(2);
        assert!(state.entries.is_empty());
        assert!(state.queue.is_empty());
    }
}
