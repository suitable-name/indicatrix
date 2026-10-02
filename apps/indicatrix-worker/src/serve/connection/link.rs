//! How a connection's adaptive-compression state ([`PeerLink`]) is built: from the
//! user's `--payload-encoding` choice, the peer's stable identity and whether the peer is
//! on this machine.

use indicatrix_net::messages::{
    PayloadEncoding,
    adaptive::{PayloadChoice, PeerLink},
};
use std::{net::SocketAddr, sync::Arc};

/// The inputs of one connection's [`PeerLink`], known before the peer's `HELLO` arrives.
#[derive(Debug, Clone)]
pub struct LinkSettings {
    /// The `--payload-encoding` choice (`auto` follows the link).
    pub choice: PayloadChoice,
    /// The peer's stable identity: the key under which its last measured bandwidth is
    /// remembered across connections (a viewer's certificate fingerprint or address, a
    /// coordinator's `host:port`).
    pub peer: String,
    /// The peer is on this machine; `auto` then sends raw payloads.
    pub loopback: bool,
}

impl LinkSettings {
    /// The link for a peer that announced `accepts` in its `HELLO`: one per connection,
    /// shared by every request on it.
    #[must_use]
    pub fn open(&self, accepts: &[PayloadEncoding]) -> Arc<PeerLink> {
        Arc::new(PeerLink::new(
            &self.peer,
            self.choice.policy(accepts, self.loopback),
        ))
    }
}

/// Whether `peer` is a loopback address (an unknown peer is not).
#[must_use]
pub fn is_loopback_peer(peer: Option<SocketAddr>) -> bool {
    peer.is_some_and(|p| p.ip().to_canonical().is_loopback())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_loopback_peer_is_raw_under_auto_and_a_pinned_choice_wins_over_it() {
        let accepts = PayloadEncoding::default_accept_list();
        let auto = LinkSettings {
            choice: PayloadChoice::Auto,
            peer: "link-settings-test:loopback".to_string(),
            loopback: true,
        }
        .open(&accepts);
        assert!(!auto.is_tracking());
        assert_eq!(auto.chosen_payload_encoding(5 << 20), PayloadEncoding::Raw);
        let pinned = LinkSettings {
            choice: PayloadChoice::Fixed(PayloadEncoding::ShuffleLz4),
            peer: "link-settings-test:pinned".to_string(),
            loopback: true,
        }
        .open(&accepts);
        assert!(!pinned.is_tracking());
        let want = if PayloadEncoding::ShuffleLz4.is_supported() {
            PayloadEncoding::ShuffleLz4
        } else {
            PayloadEncoding::Raw
        };
        assert_eq!(pinned.chosen_payload_encoding(5 << 20), want);
    }

    #[test]
    fn loopback_detection_covers_v4_v6_and_mapped_addresses() {
        let addr = |text: &str| Some(text.parse::<SocketAddr>().unwrap());
        assert!(is_loopback_peer(addr("127.0.0.1:1")));
        assert!(is_loopback_peer(addr("[::1]:1")));
        assert!(is_loopback_peer(addr("[::ffff:127.0.0.1]:1")));
        assert!(!is_loopback_peer(addr("10.0.0.2:1")));
        assert!(!is_loopback_peer(None));
    }
}
