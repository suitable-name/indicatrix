//! The user-facing payload-encoding setting: `auto` (follow the measured link) or one
//! pinned encoding. Shared by the worker's `--payload-encoding` flag and the desktop
//! app's `payload_encoding` setting.

use super::policy::AdaptiveEncoderPolicy;
use crate::messages::PayloadEncoding;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::{fmt, str::FromStr};

/// The highest zstd level the `zstd[:LEVEL]` spelling accepts.
pub const MAX_ZSTD_LEVEL: u8 = 22;

/// What a user asked for: `auto` (the default) or one pinned encoding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PayloadChoice {
    /// Follow the measured bandwidth of each connection ([`AdaptiveEncoderPolicy::adaptive`]);
    /// loopback peers get `Raw`.
    #[default]
    Auto,
    /// Always this encoding when the peer accepts it (else `Raw`), on every link,
    /// loopback included ([`AdaptiveEncoderPolicy::fixed`]).
    Fixed(PayloadEncoding),
}

impl PayloadChoice {
    /// The policy for a peer that announced `accepts`; `loopback` only matters for
    /// [`Self::Auto`].
    #[must_use]
    pub fn policy(self, accepts: &[PayloadEncoding], loopback: bool) -> AdaptiveEncoderPolicy {
        match self {
            Self::Auto => AdaptiveEncoderPolicy::adaptive(accepts, loopback),
            Self::Fixed(encoding) => AdaptiveEncoderPolicy::fixed(&[encoding], accepts),
        }
    }

    /// Whether this is [`Self::Auto`].
    #[must_use]
    pub const fn is_auto(self) -> bool {
        matches!(self, Self::Auto)
    }

    /// A `deserialize_with` function for a settings file field: an unreadable spelling
    /// (a hand-edit, a value from a newer build) loads as [`Self::Auto`] with a warning
    /// instead of failing the whole file.
    ///
    /// # Errors
    ///
    /// Only when the value is not a string at all.
    pub fn deserialize_lenient<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        Ok(text.parse().unwrap_or_else(|e| {
            tracing::warn!("ignoring the payload_encoding setting: {e}; using auto");
            Self::Auto
        }))
    }
}

impl Serialize for PayloadChoice {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for PayloadChoice {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        text.parse().map_err(serde::de::Error::custom)
    }
}

impl FromStr for PayloadChoice {
    type Err = String;

    /// Parses `auto`, `raw`, `lz4`, `zstd` (level 1) or `zstd:LEVEL` (1 to
    /// [`MAX_ZSTD_LEVEL`]), case-insensitively.
    fn from_str(text: &str) -> Result<Self, String> {
        let lower = text.trim().to_ascii_lowercase();
        match lower.as_str() {
            "auto" => Ok(Self::Auto),
            "raw" => Ok(Self::Fixed(PayloadEncoding::Raw)),
            "lz4" => Ok(Self::Fixed(PayloadEncoding::ShuffleLz4)),
            "zstd" => Ok(Self::Fixed(PayloadEncoding::DEFAULT_ZSTD)),
            other => parse_zstd_level(other)
                .map(|level| Self::Fixed(PayloadEncoding::ShuffleZstd { level }))
                .ok_or_else(|| {
                    format!("expected auto, raw, lz4, zstd or zstd:LEVEL (1 to {MAX_ZSTD_LEVEL}), got {text:?}")
                }),
        }
    }
}

/// The level of a `zstd:LEVEL` spelling, if it is one and the level is in range.
fn parse_zstd_level(lower: &str) -> Option<u8> {
    lower
        .strip_prefix("zstd:")?
        .parse::<u8>()
        .ok()
        .filter(|level| (1..=MAX_ZSTD_LEVEL).contains(level))
}

impl fmt::Display for PayloadChoice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Auto => f.write_str("auto"),
            Self::Fixed(PayloadEncoding::Raw) => f.write_str("raw"),
            Self::Fixed(PayloadEncoding::ShuffleLz4) => f.write_str("lz4"),
            Self::Fixed(PayloadEncoding::ShuffleZstd { level }) => write!(f, "zstd:{level}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_spelling_parses_and_round_trips_through_display() {
        for (text, want) in [
            ("auto", PayloadChoice::Auto),
            ("RAW", PayloadChoice::Fixed(PayloadEncoding::Raw)),
            ("lz4", PayloadChoice::Fixed(PayloadEncoding::ShuffleLz4)),
            ("zstd", PayloadChoice::Fixed(PayloadEncoding::DEFAULT_ZSTD)),
            (
                " zstd:5 ",
                PayloadChoice::Fixed(PayloadEncoding::ShuffleZstd { level: 5 }),
            ),
        ] {
            let parsed: PayloadChoice = text.parse().unwrap();
            assert_eq!(parsed, want, "{text:?}");
            assert_eq!(parsed.to_string().parse::<PayloadChoice>(), Ok(parsed));
        }
        assert_eq!(PayloadChoice::default(), PayloadChoice::Auto);
    }

    #[test]
    fn the_setting_serialises_as_its_spelling_and_a_bad_value_loads_as_auto() {
        #[derive(Serialize, Deserialize, PartialEq, Debug)]
        struct Doc {
            #[serde(default, deserialize_with = "PayloadChoice::deserialize_lenient")]
            payload_encoding: PayloadChoice,
        }
        let pinned = Doc {
            payload_encoding: PayloadChoice::Fixed(PayloadEncoding::ShuffleZstd { level: 5 }),
        };
        let bytes = postcard::to_allocvec(&pinned).unwrap();
        assert_eq!(bytes, postcard::to_allocvec("zstd:5").unwrap());
        assert_eq!(postcard::from_bytes::<Doc>(&bytes).unwrap(), pinned);
        let bad = postcard::to_allocvec("gzip").unwrap();
        let doc: Doc = postcard::from_bytes(&bad).unwrap();
        assert_eq!(doc.payload_encoding, PayloadChoice::Auto);
    }

    #[test]
    fn bad_spellings_and_levels_are_refused() {
        for bad in ["", "gzip", "zstd:", "zstd:0", "zstd:23", "zstd:x", "lz4:3"] {
            assert!(bad.parse::<PayloadChoice>().is_err(), "{bad:?}");
        }
    }

    #[test]
    fn a_fixed_choice_pins_the_policy_and_auto_follows_the_link() {
        let accepts = PayloadEncoding::default_accept_list();
        let fixed = PayloadChoice::Fixed(PayloadEncoding::Raw).policy(&accepts, false);
        assert!(!fixed.is_adaptive());
        let auto = PayloadChoice::Auto.policy(&accepts, true);
        assert!(auto.is_adaptive() && auto.is_loopback());
    }
}
