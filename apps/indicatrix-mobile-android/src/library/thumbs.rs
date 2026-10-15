//! Thumbnails for the cards near the visible range.
//!
//! Making a thumbnail solves and rasterizes a design, so it runs on two worker threads of
//! its own (a slow thumbnail must not hold up a search). The UI thread keeps the list of
//! thumbnails it wants, nearest to the top of the screen first; a worker always takes the
//! newest wish, and a new list replaces the old one, so a card that was scrolled away
//! before its turn is never made.
//!
//! Images live in the card rows. At most [`MAX_LOADED`] rows hold one: when more arrive,
//! the ones farthest from the visible range are dropped again.

use std::{
    collections::HashSet,
    panic::{AssertUnwindSafe, catch_unwind},
    sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError},
    thread::JoinHandle,
};

use indicatrix_mobile_core::{
    library::{EntryId, LibraryHandle, Thumbnail},
    tasks::join_briefly,
};
use slint::{Image, Rgba8Pixel, SharedPixelBuffer, Weak};

use crate::AppWindow;

/// How many worker threads make thumbnails.
const WORKERS: usize = 2;
/// How many rows may hold an image at once.
pub(super) const MAX_LOADED: usize = 160;
/// How many cards beyond the visible range get a thumbnail ahead of the scroll.
pub(super) const PREFETCH: usize = 8;
/// The long edge of a grid thumbnail in pixels.
pub(super) const GRID_EDGE: u32 = 256;
/// The long edge of a list thumbnail in pixels.
pub(super) const LIST_EDGE: u32 = 128;

/// One thumbnail the UI wants.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Job {
    /// The library entry.
    pub id: EntryId,
    /// The card's index when the job was made (a hint; the card is found by id).
    pub index: usize,
    /// The long edge in pixels.
    pub edge: u32,
}

/// The wishes and the work in progress, behind one lock.
#[derive(Default)]
struct PoolState {
    /// The wishes; the last entry is made first.
    wanted: Vec<Job>,
    /// The entries a worker is making now.
    in_flight: HashSet<EntryId>,
    stop: bool,
}

struct Shared {
    state: Mutex<PoolState>,
    wake: Condvar,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, PoolState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// The worker threads. Dropping the pool stops and joins them (waiting at most two
/// seconds each).
pub(super) struct ThumbPool {
    shared: Arc<Shared>,
    workers: Vec<JoinHandle<()>>,
}

impl ThumbPool {
    /// Starts the workers. Results are delivered to the UI thread through `weak`.
    pub fn start(handle: &LibraryHandle, weak: &Weak<AppWindow>) -> Self {
        let shared = Arc::new(Shared {
            state: Mutex::new(PoolState::default()),
            wake: Condvar::new(),
        });
        let mut workers = Vec::with_capacity(WORKERS);
        for number in 0..WORKERS {
            let (shared, handle, weak) = (Arc::clone(&shared), handle.clone(), weak.clone());
            let spawned = std::thread::Builder::new()
                .name(format!("library-thumb-{number}"))
                .spawn(move || work(&shared, &handle, &weak));
            match spawned {
                Ok(join) => workers.push(join),
                Err(error) => tracing::warn!(%error, "a thumbnail worker could not start"),
            }
        }
        Self { shared, workers }
    }

    /// Replaces the wishes. The job to make first goes last.
    pub fn set_wanted(&self, jobs: Vec<Job>) {
        self.shared.lock().wanted = jobs;
        self.shared.wake.notify_all();
    }

    /// Stops the workers and waits briefly for them.
    pub fn shutdown(&mut self) {
        {
            let mut state = self.shared.lock();
            state.stop = true;
            state.wanted.clear();
        }
        self.shared.wake.notify_all();
        for worker in self.workers.drain(..) {
            join_briefly(worker, "library-thumb");
        }
    }
}

impl Drop for ThumbPool {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// A worker's loop: take the newest wish, make it, hand it to the UI thread.
fn work(shared: &Shared, handle: &LibraryHandle, weak: &Weak<AppWindow>) {
    loop {
        let job = {
            let mut guard = shared.lock();
            loop {
                if guard.stop {
                    return;
                }
                let PoolState {
                    wanted, in_flight, ..
                } = &mut *guard;
                if let Some(position) = wanted.iter().rposition(|job| !in_flight.contains(&job.id))
                {
                    let job = wanted.remove(position);
                    in_flight.insert(job.id);
                    break job;
                }
                guard = shared
                    .wake
                    .wait(guard)
                    .unwrap_or_else(PoisonError::into_inner);
            }
        };
        let made = catch_unwind(AssertUnwindSafe(|| handle.thumbnail_rgba(job.id, job.edge)))
            .unwrap_or_else(|_| Err("The thumbnail could not be made.".to_owned()));
        shared.lock().in_flight.remove(&job.id);
        let delivered = weak.upgrade_in_event_loop(move |_window| {
            super::list::thumbnail_ready(&job, made);
        });
        if let Err(error) = delivered {
            tracing::debug!(%error, "a thumbnail was dropped: the UI thread is gone");
        }
    }
}

/// The picture of a thumbnail, `None` when its pixel data does not match its size.
pub(super) fn to_image(thumbnail: &Thumbnail) -> Option<Image> {
    let pixels = u64::from(thumbnail.width) * u64::from(thumbnail.height) * 4;
    if thumbnail.width == 0 || thumbnail.height == 0 || thumbnail.rgba.len() as u64 != pixels {
        return None;
    }
    let mut buffer = SharedPixelBuffer::<Rgba8Pixel>::new(thumbnail.width, thumbnail.height);
    let bytes = buffer.make_mut_bytes();
    if bytes.len() != thumbnail.rgba.len() {
        return None;
    }
    bytes.copy_from_slice(&thumbnail.rgba);
    Some(Image::from_rgba8(buffer))
}

/// The wishes for the cards `first..=last` (the visible range) and a margin around it,
/// skipping the cards that already have an image or failed. The result is ordered for
/// [`ThumbPool::set_wanted`]: the card nearest to `first` comes last.
pub(super) fn wishes(
    len: usize,
    id_at: &dyn Fn(usize) -> EntryId,
    range: (usize, usize),
    skip: &dyn Fn(usize, EntryId) -> bool,
    edge: u32,
) -> Vec<Job> {
    if len == 0 {
        return Vec::new();
    }
    let (first, last) = (range.0.min(len - 1), range.1.min(len - 1));
    let low = first.saturating_sub(PREFETCH);
    let high = (last + PREFETCH).min(len - 1);
    let mut indices: Vec<usize> = (low..=high)
        .filter(|index| !skip(*index, id_at(*index)))
        .collect();
    // Visible cards by distance from the top of the screen, then the margin by distance
    // from the visible range.
    indices.sort_by_key(|index| {
        if (first..=last).contains(index) {
            index - first
        } else {
            (last - first + 1) + index.abs_diff(if *index < first { first } else { last })
        }
    });
    indices.reverse();
    indices
        .into_iter()
        .map(|index| Job {
            id: id_at(index),
            index,
            edge,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn never(_: usize, _: EntryId) -> bool {
        false
    }

    #[test]
    fn wishes_put_the_top_of_the_screen_last() {
        let cards: Vec<EntryId> = (100..140).collect();
        let jobs = wishes(cards.len(), &|i| cards[i], (10, 14), &never, GRID_EDGE);
        assert_eq!(jobs.last().map(|job| job.index), Some(10));
        assert_eq!(jobs[jobs.len() - 5].index, 14);
        // The margin comes first (made last): 8 above and 8 below the range.
        assert_eq!(jobs.len(), 5 + 2 * PREFETCH);
        assert!(jobs.iter().all(|job| (2..=22).contains(&job.index)));
        assert!(jobs.iter().all(|job| job.id == cards[job.index]));
    }

    #[test]
    fn wishes_skip_loaded_cards_and_clip_to_the_list() {
        let cards: Vec<EntryId> = (0..5).collect();
        let jobs = wishes(
            cards.len(),
            &|i| cards[i],
            (0, 40),
            &|index, _| index == 1,
            LIST_EDGE,
        );
        let indices: Vec<usize> = jobs.iter().map(|job| job.index).collect();
        assert_eq!(indices, [4, 3, 2, 0]);
        assert!(jobs.iter().all(|job| job.edge == LIST_EDGE));
        assert_eq!(wishes(0, &|_| 0, (0, 3), &never, GRID_EDGE).len(), 0);
    }

    #[test]
    fn to_image_checks_the_pixel_count() {
        let good = Thumbnail {
            width: 2,
            height: 1,
            rgba: vec![0; 8],
        };
        assert!(to_image(&good).is_some());
        let short = Thumbnail {
            width: 2,
            height: 2,
            rgba: vec![0; 8],
        };
        assert!(to_image(&short).is_none());
        let empty = Thumbnail {
            width: 0,
            height: 0,
            rgba: Vec::new(),
        };
        assert!(to_image(&empty).is_none());
    }
}
