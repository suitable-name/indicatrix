//! The handshake messages: `-> HELLO` / `<- WELCOME`.
//!
//! [`Welcome`] is where a worker honestly advertises what it can actually do --
//! [`Welcome::render`] is `Some` iff this build has active render capacity, `None` for
//! a library-only worker. A client must check this BEFORE ever sending a
//! `RenderRequest`: on a library-only build, that message type doesn't even exist, so
//! sending one anyway fails to decode rather than being gracefully refused.
//! [`Welcome::library`] is the mirror-image signal for the library protocol, kept
//! explicit (rather than assumed always-true) so a future build that can disable it
//! doesn't need a breaking `Welcome` change. [`Welcome::tilt_curves`] follows the same
//! pattern for `TILT_CURVES` -- checked before ever sending a `TiltCurvesRequest`.
//!
//! # `build_hash` vs `source_hash`
//!
//! Both [`Hello`] and [`Welcome`] carry two independent `indicatrix` identity fields --
//! see `crate::handshake`'s module doc comment for the full two-level check
//! [`crate::handshake::verify_compatible`] runs against them.

use serde::{Deserialize, Serialize};

/// `-> HELLO`: a client's opening message, identifying its protocol version and
/// `indicatrix` build.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hello {
    pub protocol_version: u16,
    /// A hash of `indicatrix`'s crate VERSION -- see [`crate::handshake::local_build_hash`].
    pub build_hash: [u8; 8],
    /// A content hash of `indicatrix`'s actual source tree -- see
    /// [`crate::handshake::local_source_hash`]. `crate::handshake::UNKNOWN_BUILD_HASH`
    /// when this side has no `indicatrix` build to report (a library-only build) or
    /// its source hash could not be established.
    pub source_hash: [u8; 8],
}

/// Which compute backend a worker is rendering on, reported in [`RenderCapability`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Backend {
    Cpu { threads: u32 },
    Gpu { adapter: String },
}

/// The render half of [`Welcome`], present only when this worker was built with (and
/// has active) render capacity -- see [`Welcome::render`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RenderCapability {
    pub backend: Backend,
    /// The largest `width * height` this worker is willing to render in one `RENDER`
    /// request.
    pub max_pixels: u32,
    /// This worker's cadence FLOOR, in milliseconds -- the fastest `StreamConfig::cadence_ms`
    /// it can usefully attempt. Purely advisory: a client may request a smaller
    /// `cadence_ms` anyway (rate-limited naturally by delta coalescing under
    /// backpressure), but a viewer UI can use this to grey out unreachable values.
    pub min_cadence_ms: u32,
}

/// `<- WELCOME`: a worker's reply to `HELLO`, identifying itself and honestly
/// advertising what it can actually do -- see the module doc comment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Welcome {
    pub protocol_version: u16,
    /// See [`Hello::build_hash`].
    pub build_hash: [u8; 8],
    /// See [`Hello::source_hash`].
    pub source_hash: [u8; 8],
    /// `Some` iff this worker can accept a `RenderRequest` right now -- check this
    /// before ever sending one. See the module doc comment.
    pub render: Option<RenderCapability>,
    /// Whether this worker serves the read-only design-library protocol
    /// (`super::super::library`). Always `true` in this phase -- see the module doc
    /// comment.
    pub library: bool,
    /// `true` iff this worker can accept a `TiltCurvesRequest` right now -- check this
    /// before ever sending one, exactly as [`Self::render`] gates `RenderRequest`.
    ///
    /// Always equal to `render.is_some()` in this phase (both share the same feature
    /// gate), but kept as its own explicit field, mirroring [`Self::library`], so a
    /// future worker that decouples `TILT_CURVES` from full render capacity doesn't
    /// need a breaking `Welcome` change.
    pub tilt_curves: bool,
}

#[cfg(test)]
mod tests {
    use super::{
        super::{
            PROTOCOL_VERSION,
            codec::{read_message, write_message},
        },
        *,
    };

    #[test]
    fn hello_round_trips() {
        let hello = Hello {
            protocol_version: PROTOCOL_VERSION,
            build_hash: [1, 2, 3, 4, 5, 6, 7, 8],
            source_hash: [8, 7, 6, 5, 4, 3, 2, 1],
        };
        let mut buf = Vec::new();
        write_message(&mut buf, &hello).unwrap();
        let mut cursor = std::io::Cursor::new(buf);
        let decoded: Hello = read_message(&mut cursor).unwrap();
        assert_eq!(hello, decoded);
    }

    #[test]
    fn welcome_round_trips_a_render_capable_worker_both_backend_variants() {
        for backend in [
            Backend::Cpu { threads: 16 },
            Backend::Gpu {
                adapter: "RTX 4090".to_string(),
            },
        ] {
            let welcome = Welcome {
                protocol_version: PROTOCOL_VERSION,
                build_hash: [9; 8],
                source_hash: [10; 8],
                render: Some(RenderCapability {
                    backend,
                    max_pixels: 8_294_400,
                    min_cadence_ms: 100,
                }),
                library: true,
                tilt_curves: true,
            };
            let mut buf = Vec::new();
            write_message(&mut buf, &welcome).unwrap();
            let mut cursor = std::io::Cursor::new(buf);
            let decoded: Welcome = read_message(&mut cursor).unwrap();
            assert_eq!(welcome, decoded);
        }
    }

    #[test]
    fn welcome_round_trips_a_library_only_worker_with_no_render_capability() {
        let welcome = Welcome {
            protocol_version: PROTOCOL_VERSION,
            build_hash: [9; 8],
            source_hash: [10; 8],
            render: None,
            library: true,
            tilt_curves: false,
        };
        let mut buf = Vec::new();
        write_message(&mut buf, &welcome).unwrap();
        let mut cursor = std::io::Cursor::new(buf);
        let decoded: Welcome = read_message(&mut cursor).unwrap();
        assert_eq!(welcome, decoded);
        assert!(decoded.render.is_none());
        assert!(decoded.library);
        assert!(!decoded.tilt_curves);
    }
}
