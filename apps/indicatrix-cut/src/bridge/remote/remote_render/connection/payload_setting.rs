//! The desktop's `payload_encoding` setting as the connection layer reads it, and the
//! per-connection [`PeerLink`] a contribution upload is sent through.
//!
//! The setting is applied once at startup (`gui::startup_settings`); the one-shot
//! connections of an export read it when they upload their share, so it needs no
//! plumbing through every request type.

use indicatrix_net::messages::{
    PayloadEncoding,
    adaptive::{PayloadChoice, PeerLink},
};
use std::{
    net::IpAddr,
    sync::{Mutex, PoisonError},
};

static CHOICE: Mutex<PayloadChoice> = Mutex::new(PayloadChoice::Auto);

/// Applies the `payload_encoding` setting: how the viewer compresses the uploads it sends
/// a coordinator (`auto` follows the measured link speed).
pub fn set_payload_choice(choice: PayloadChoice) {
    *CHOICE.lock().unwrap_or_else(PoisonError::into_inner) = choice;
}

/// The current `payload_encoding` setting.
#[must_use]
pub fn payload_choice() -> PayloadChoice {
    *CHOICE.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Whether `address` (`host:port`) names this machine.
fn is_loopback_address(address: &str) -> bool {
    let host = address.rsplit_once(':').map_or(address, |(host, _)| host);
    let host = host.trim_start_matches('[').trim_end_matches(']');
    host.eq_ignore_ascii_case("localhost")
        || host.parse::<IpAddr>().is_ok_and(|ip| ip.is_loopback())
}

/// The link for one connection to the coordinator at `address`: keyed by that address (so
/// the next connection starts from this one's measured speed), following the current
/// setting. This build runs on both ends of a connection (the handshake checked the build
/// hash), so the coordinator decodes every encoding this one can encode.
#[must_use]
pub fn open_link(address: &str) -> PeerLink {
    PeerLink::new(
        address,
        payload_choice().policy(
            &PayloadEncoding::default_accept_list(),
            is_loopback_address(address),
        ),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loopback_addresses_are_recognised_in_every_spelling() {
        for address in [
            "127.0.0.1:7878",
            "localhost:7878",
            "[::1]:7878",
            "LOCALHOST:1",
        ] {
            assert!(is_loopback_address(address), "{address}");
        }
        for address in [
            "10.0.0.5:7878",
            "coordinator.lan:7878",
            "[fe80::1]:7878",
            "",
        ] {
            assert!(!is_loopback_address(address), "{address}");
        }
    }

    #[test]
    fn the_setting_round_trips_and_defaults_to_auto() {
        let before = payload_choice();
        set_payload_choice(PayloadChoice::Fixed(PayloadEncoding::ShuffleLz4));
        assert_eq!(
            payload_choice(),
            PayloadChoice::Fixed(PayloadEncoding::ShuffleLz4)
        );
        set_payload_choice(before);
    }
}
