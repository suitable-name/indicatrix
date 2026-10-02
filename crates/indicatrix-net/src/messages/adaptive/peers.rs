//! The last known bandwidth of recently seen peers, so a new connection starts on a good
//! tier instead of the default one. In memory only, deterministic (`BTreeMap`), bounded.

use std::{
    collections::BTreeMap,
    sync::{Mutex, PoisonError},
};

/// How many peers the process-wide book remembers before it forgets the oldest.
pub const MAX_REMEMBERED_PEERS: usize = 64;

/// A bounded map from a peer key to its last bandwidth estimate in Mbit/s.
///
/// Every `record` stamps the entry with an increasing sequence number; at capacity the
/// entry with the smallest stamp (the least recently recorded) is evicted.
#[derive(Debug, Clone)]
pub struct PeerBandwidthBook {
    entries: BTreeMap<String, (f64, u64)>,
    next_stamp: u64,
    capacity: usize,
}

impl PeerBandwidthBook {
    /// An empty book holding at most `capacity` peers (at least one).
    #[must_use]
    pub const fn new(capacity: usize) -> Self {
        Self {
            entries: BTreeMap::new(),
            next_stamp: 0,
            capacity: if capacity == 0 { 1 } else { capacity },
        }
    }

    /// Remembers `mbps` for `key`. Non-finite and non-positive values are ignored.
    pub fn record(&mut self, key: &str, mbps: f64) {
        if !mbps.is_finite() || mbps <= 0.0 {
            return;
        }
        if !self.entries.contains_key(key) && self.entries.len() >= self.capacity {
            let oldest = self
                .entries
                .iter()
                .min_by_key(|(_, (_, stamp))| *stamp)
                .map(|(k, _)| k.clone());
            if let Some(oldest) = oldest {
                self.entries.remove(&oldest);
            }
        }
        self.entries
            .insert(key.to_string(), (mbps, self.next_stamp));
        self.next_stamp += 1;
    }

    /// The last bandwidth recorded for `key`.
    #[must_use]
    pub fn lookup(&self, key: &str) -> Option<f64> {
        self.entries.get(key).map(|(mbps, _)| *mbps)
    }

    /// How many peers are remembered.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether nothing is remembered.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// The process-wide book every connection seeds from and reports to.
static GLOBAL_BOOK: Mutex<PeerBandwidthBook> =
    Mutex::new(PeerBandwidthBook::new(MAX_REMEMBERED_PEERS));

/// Remembers `mbps` for `key` in the process-wide book.
pub fn remember_peer_bandwidth(key: &str, mbps: f64) {
    GLOBAL_BOOK
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .record(key, mbps);
}

/// The last bandwidth the process-wide book holds for `key`.
#[must_use]
pub fn recall_peer_bandwidth(key: &str) -> Option<f64> {
    GLOBAL_BOOK
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .lookup(key)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_latest_value_wins_and_unusable_values_are_ignored() {
        let mut book = PeerBandwidthBook::new(4);
        book.record("a", 100.0);
        book.record("a", 250.0);
        book.record("b", f64::NAN);
        book.record("b", -3.0);
        assert_eq!(book.lookup("a"), Some(250.0));
        assert_eq!(book.lookup("b"), None);
        assert_eq!(book.len(), 1);
    }

    #[test]
    fn the_least_recently_recorded_peer_is_evicted_at_capacity() {
        let mut book = PeerBandwidthBook::new(2);
        book.record("a", 1.0);
        book.record("b", 2.0);
        book.record("a", 3.0); // refreshes "a": "b" is now the oldest
        book.record("c", 4.0);
        assert_eq!(book.lookup("b"), None);
        assert_eq!(book.lookup("a"), Some(3.0));
        assert_eq!(book.lookup("c"), Some(4.0));
        assert_eq!(book.len(), 2);
    }

    #[test]
    fn the_process_wide_book_round_trips() {
        remember_peer_bandwidth("peers-test:global", 640.0);
        assert_eq!(recall_peer_bandwidth("peers-test:global"), Some(640.0));
        assert_eq!(recall_peer_bandwidth("peers-test:never-seen"), None);
    }
}
