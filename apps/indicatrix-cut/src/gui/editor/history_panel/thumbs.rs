//! What names a thumbnail, and the cache that holds the finished ones.
//!
//! A step's picture is the design at that step, so it only has to be drawn again when that
//! design changes. [`ThumbKey`] says exactly that: the design (its `epoch`: a different New /
//! Open starts over), the step (its `position`, its `revision`, which moves whenever more
//! nudges merge into the newest step, and its words) and the size it is drawn at. Undoing
//! and redoing leave the revisions alone, so a picture drawn before a jump is still good
//! after it. Everything here is plain data, so the tests cover it directly.

use indicatrix_cut_core::HistoryEntry;
use std::collections::HashMap;

use super::rows::START_TITLE;

/// The picture's size in logical pixels (it is drawn at this times the window's scale).
pub(super) const LOGICAL_EDGE: u32 = 72;

/// The smallest and the largest edge a picture is drawn at, in physical pixels: a guard
/// against an odd scale factor, far from any real window.
const MIN_EDGE_PX: u32 = 16;
const MAX_EDGE_PX: u32 = 512;

/// The revision of the "Start" row's picture. The start of a history never changes, so it
/// needs no real revision; this is a value no step can have.
pub(super) const START_REVISION: u64 = u64::MAX;

/// The most pictures the cache keeps. A 72 pixel picture at scale 2 is about 83 KB.
pub(super) const CACHE_CAPACITY: usize = 256;

/// Names one picture.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(super) struct ThumbKey {
    /// The editor's design epoch: a different design than before is a different picture.
    pub(super) epoch: u64,
    /// The step (0 is "Start").
    pub(super) position: usize,
    /// The step's revision, see `HistoryEntry::revision`.
    pub(super) revision: u64,
    /// The step's words.
    pub(super) label: String,
    /// The picture's edge in physical pixels.
    pub(super) edge_px: u32,
}

/// The picture edge in physical pixels at window scale factor `scale`.
#[must_use]
pub(super) fn edge_px(scale: f32) -> u32 {
    let scale = if scale.is_finite() && scale > 0.0 {
        scale
    } else {
        1.0
    };
    ((LOGICAL_EDGE as f32 * scale).round() as u32).clamp(MIN_EDGE_PX, MAX_EDGE_PX)
}

/// The key of every row, indexed by position: `keys[0]` is "Start", `keys[p]` step `p`.
/// `entries` is oldest first with positions 1, 2, 3, ... as `History::entries` returns it.
#[must_use]
pub(super) fn thumb_keys(epoch: u64, entries: &[HistoryEntry], edge_px: u32) -> Vec<ThumbKey> {
    let start = ThumbKey {
        epoch,
        position: 0,
        revision: START_REVISION,
        label: START_TITLE.to_string(),
        edge_px,
    };
    std::iter::once(start)
        .chain(entries.iter().map(|entry| ThumbKey {
            epoch,
            position: entry.position,
            revision: entry.revision,
            label: entry.label.clone(),
            edge_px,
        }))
        .collect()
}

/// One cached picture's state.
#[derive(Debug, Clone)]
enum Slot<V> {
    /// Asked for and not back yet.
    Waiting,
    /// Drawn.
    Ready { value: V, used: u64 },
    /// The design at that step cannot be drawn (it does not solve).
    Failed { used: u64 },
}

/// What a lookup learns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Lookup<V> {
    /// The picture.
    Ready(V),
    /// Nothing to draw: the design at that step does not solve.
    Failed,
    /// Being drawn already.
    Waiting,
    /// Not known and now marked as waiting: the caller must queue it.
    Requested,
}

/// Finished pictures by key, least recently used out first beyond a capacity.
#[derive(Debug)]
pub(super) struct ThumbCache<V> {
    slots: HashMap<ThumbKey, Slot<V>>,
    /// Counts lookups and stores; unique per use, so eviction never depends on hash order.
    tick: u64,
    capacity: usize,
}

impl<V: Clone> ThumbCache<V> {
    /// An empty cache holding at most `capacity` finished pictures.
    #[must_use]
    pub(super) fn new(capacity: usize) -> Self {
        Self {
            slots: HashMap::new(),
            tick: 0,
            capacity,
        }
    }

    /// Answers a row's lookup. A key not known yet is marked waiting and reported as
    /// [`Lookup::Requested`], once: the caller queues it, and later lookups say
    /// [`Lookup::Waiting`] until the picture is stored.
    pub(super) fn lookup(&mut self, key: &ThumbKey) -> Lookup<V> {
        self.tick += 1;
        let tick = self.tick;
        match self.slots.get_mut(key) {
            Some(Slot::Ready { value, used }) => {
                *used = tick;
                Lookup::Ready(value.clone())
            }
            Some(Slot::Failed { used }) => {
                *used = tick;
                Lookup::Failed
            }
            Some(Slot::Waiting) => Lookup::Waiting,
            None => {
                self.slots.insert(key.clone(), Slot::Waiting);
                Lookup::Requested
            }
        }
    }

    /// Stores a finished picture.
    pub(super) fn store_ready(&mut self, key: ThumbKey, value: V) {
        self.tick += 1;
        let used = self.tick;
        self.slots.insert(key, Slot::Ready { value, used });
        self.evict();
    }

    /// Records that the design at the key's step cannot be drawn.
    pub(super) fn store_failed(&mut self, key: ThumbKey) {
        self.tick += 1;
        let used = self.tick;
        self.slots.insert(key, Slot::Failed { used });
        self.evict();
    }

    /// Forgets a request that came to nothing (dropped from a full queue, or answered for a
    /// design that moved on), so the next lookup asks again. A finished picture stays.
    pub(super) fn forget_waiting(&mut self, key: &ThumbKey) {
        if matches!(self.slots.get(key), Some(Slot::Waiting)) {
            self.slots.remove(key);
        }
    }

    /// Drops everything that belongs to another design than `epoch`.
    pub(super) fn retain_epoch(&mut self, epoch: u64) {
        self.slots.retain(|key, _| key.epoch == epoch);
    }

    /// How many slots there are, waiting ones included.
    #[cfg(test)]
    pub(super) fn len(&self) -> usize {
        self.slots.len()
    }

    /// Removes the least recently used finished slot while more than the capacity are held.
    /// Waiting slots do not count: the work queue bounds those.
    fn evict(&mut self) {
        let finished = |slot: &Slot<V>| !matches!(slot, Slot::Waiting);
        while self.slots.values().filter(|slot| finished(slot)).count() > self.capacity {
            let oldest = self
                .slots
                .iter()
                .filter_map(|(key, slot)| match slot {
                    Slot::Ready { used, .. } | Slot::Failed { used } => Some((*used, key.clone())),
                    Slot::Waiting => None,
                })
                .min_by_key(|(used, _)| *used);
            match oldest {
                Some((_, key)) => {
                    self.slots.remove(&key);
                }
                None => break,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(position: usize, revision: u64) -> ThumbKey {
        ThumbKey {
            epoch: 1,
            position,
            revision,
            label: format!("step {position}"),
            edge_px: 72,
        }
    }

    fn entry(position: usize, label: &str, revision: u64) -> HistoryEntry {
        HistoryEntry {
            label: label.to_string(),
            position,
            revision,
            undone: false,
        }
    }

    #[test]
    fn the_edge_follows_the_window_scale_and_stays_in_bounds() {
        assert_eq!(edge_px(1.0), 72);
        assert_eq!(edge_px(2.0), 144);
        assert_eq!(edge_px(1.5), 108);
        assert_eq!(edge_px(0.0), 72, "a zero scale reads as 1");
        assert_eq!(edge_px(f32::NAN), 72);
        assert_eq!(edge_px(100.0), 512);
        assert_eq!(edge_px(0.01), 16);
    }

    #[test]
    fn keys_are_indexed_by_position_with_start_first() {
        let keys = thumb_keys(
            7,
            &[entry(1, "Add tier T", 4), entry(2, "Add tier P1", 5)],
            144,
        );
        assert_eq!(keys.len(), 3);
        for (index, key) in keys.iter().enumerate() {
            assert_eq!(key.position, index);
            assert_eq!(key.epoch, 7);
            assert_eq!(key.edge_px, 144);
        }
        assert_eq!(keys[0].revision, START_REVISION);
        assert_eq!(keys[0].label, "Start");
        assert_eq!(
            (keys[2].revision, keys[2].label.as_str()),
            (5, "Add tier P1")
        );
    }

    #[test]
    fn a_picture_is_drawn_again_when_the_step_changes_but_not_when_it_is_undone() {
        // Merged nudges move the newest step's revision (and words): a different key.
        let before = thumb_keys(1, &[entry(1, "Set P1 to 41.0", 3)], 72);
        let merged = thumb_keys(1, &[entry(1, "Set P1 to 41.5", 4)], 72);
        assert_ne!(before[1], merged[1]);
        // Undoing the step changes neither its revision nor its words: the same key.
        let mut undone = entry(1, "Set P1 to 41.0", 3);
        undone.undone = true;
        assert_eq!(before[1], thumb_keys(1, &[undone], 72)[1]);
        // A new design (another epoch) or another size is a different picture.
        assert_ne!(
            before[1],
            thumb_keys(2, &[entry(1, "Set P1 to 41.0", 3)], 72)[1]
        );
        assert_ne!(
            before[1],
            thumb_keys(1, &[entry(1, "Set P1 to 41.0", 3)], 144)[1]
        );
    }

    #[test]
    fn a_miss_is_requested_once_then_waits_then_hits() {
        let mut cache = ThumbCache::<u32>::new(8);
        let k = key(1, 1);
        assert_eq!(cache.lookup(&k), Lookup::Requested);
        assert_eq!(cache.lookup(&k), Lookup::Waiting);
        cache.store_ready(k.clone(), 42);
        assert_eq!(cache.lookup(&k), Lookup::Ready(42));
    }

    #[test]
    fn a_design_that_does_not_solve_is_remembered_as_failed() {
        let mut cache = ThumbCache::<u32>::new(8);
        let k = key(2, 2);
        assert_eq!(cache.lookup(&k), Lookup::Requested);
        cache.store_failed(k.clone());
        assert_eq!(cache.lookup(&k), Lookup::Failed);
    }

    #[test]
    fn a_forgotten_request_is_asked_for_again_but_a_finished_picture_stays() {
        let mut cache = ThumbCache::<u32>::new(8);
        let waiting = key(1, 1);
        let done = key(2, 2);
        cache.lookup(&waiting);
        cache.store_ready(done.clone(), 7);
        cache.forget_waiting(&waiting);
        cache.forget_waiting(&done);
        assert_eq!(cache.lookup(&waiting), Lookup::Requested);
        assert_eq!(cache.lookup(&done), Lookup::Ready(7));
    }

    #[test]
    fn the_least_recently_used_picture_goes_first() {
        let mut cache = ThumbCache::<u32>::new(3);
        for n in 0..3 {
            cache.store_ready(key(n, n as u64), n as u32);
        }
        // Using picture 0 makes picture 1 the oldest.
        assert_eq!(cache.lookup(&key(0, 0)), Lookup::Ready(0));
        cache.store_ready(key(3, 3), 3);
        assert_eq!(cache.lookup(&key(0, 0)), Lookup::Ready(0));
        assert_eq!(
            cache.lookup(&key(1, 1)),
            Lookup::Requested,
            "picture 1 was evicted"
        );
        assert_eq!(cache.lookup(&key(3, 3)), Lookup::Ready(3));
    }

    #[test]
    fn waiting_slots_do_not_count_against_the_capacity() {
        let mut cache = ThumbCache::<u32>::new(1);
        for n in 0..5 {
            assert_eq!(cache.lookup(&key(n, n as u64)), Lookup::Requested);
        }
        cache.store_ready(key(0, 0), 1);
        assert_eq!(cache.len(), 5);
        assert_eq!(cache.lookup(&key(0, 0)), Lookup::Ready(1));
    }

    #[test]
    fn a_new_design_drops_the_old_ones_pictures() {
        let mut cache = ThumbCache::<u32>::new(8);
        let old = key(1, 1);
        let mut new = key(1, 1);
        new.epoch = 2;
        cache.store_ready(old.clone(), 1);
        cache.store_ready(new.clone(), 2);
        cache.retain_epoch(2);
        assert_eq!(cache.lookup(&old), Lookup::Requested);
        assert_eq!(cache.lookup(&new), Lookup::Ready(2));
    }
}
