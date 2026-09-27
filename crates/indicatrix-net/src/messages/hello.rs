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
//!
//! # Roles (v14)
//!
//! [`Hello::role`] says who is dialing: a [`PeerRole::Viewer`] (the GUI, or a coordinator
//! acting as a viewer toward nobody) wants to send requests; a [`PeerRole::Worker`] is a
//! render node dialing OUT to a coordinator (`indicatrix-worker join`) and, after the
//! handshake, answers the coordinator's requests instead of sending its own. A worker
//! Hello carries its [`RenderCapability`] in [`Hello::capability`]; a coordinator that
//! accepts it replies with [`Welcome::registration`] set. A server that does not accept
//! joining workers refuses a worker Hello with an `ErrorMsg` in place of `WELCOME`.
//!
//! # The first three fields are the version-probe prefix
//!
//! `protocol_version`, `build_hash`, `source_hash` are, and must stay, the first three
//! fields of both [`Hello`] and [`Welcome`], in that order: they are exactly a v12/v13
//! `HELLO`, so a peer can always decode that prefix (see
//! [`crate::handshake::read_hello`]) and report a clear version mismatch instead of a
//! postcard decode error, whichever side is older.

use super::encoding::PayloadEncoding;
use serde::{Deserialize, Serialize};

/// Who sent a [`Hello`] -- see the module doc comment's "Roles" section. Variant order
/// is wire-load-bearing; append only.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PeerRole {
    /// A viewer (the GUI): sends requests, reads replies.
    Viewer,
    /// A render worker joining a coordinator: dials out, then serves the coordinator's
    /// requests on the same connection.
    Worker,
}

/// `-> HELLO`: a client's opening message, identifying its protocol version and
/// `indicatrix` build, its role, and which payload encodings it can decode.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hello {
    /// This peer's [`super::PROTOCOL_VERSION`]. Must stay the first field (module doc).
    pub protocol_version: u16,
    /// A hash of `indicatrix`'s crate VERSION -- see [`crate::handshake::local_build_hash`].
    pub build_hash: [u8; 8],
    /// A content hash of `indicatrix`'s actual source tree -- see
    /// [`crate::handshake::local_source_hash`]. `crate::handshake::UNKNOWN_BUILD_HASH`
    /// when this side has no `indicatrix` build to report (a library-only build) or
    /// its source hash could not be established.
    pub source_hash: [u8; 8],
    /// Who is dialing -- see the module doc comment's "Roles" section.
    pub role: PeerRole,
    /// `Some` iff [`Self::role`] is [`PeerRole::Worker`]: what the joining worker can
    /// render. A server rejects a Hello whose role and capability disagree
    /// ([`Self::role_is_consistent`]).
    pub capability: Option<RenderCapability>,
    /// The payload encodings this peer can DECODE, in its own order of preference
    /// (informational; the server's preference decides). `Raw` is always implicitly
    /// accepted. See `super::encoding`'s module doc comment.
    pub accept_encodings: Vec<PayloadEncoding>,
}

impl Hello {
    /// A viewer `HELLO` carrying just a build identity, accepting this build's
    /// [`PayloadEncoding::default_accept_list`].
    #[must_use]
    pub fn viewer(protocol_version: u16, build_hash: [u8; 8], source_hash: [u8; 8]) -> Self {
        Self {
            protocol_version,
            build_hash,
            source_hash,
            role: PeerRole::Viewer,
            capability: None,
            accept_encodings: PayloadEncoding::default_accept_list(),
        }
    }

    /// Whether [`Self::role`] and [`Self::capability`] agree: a worker must report its
    /// capability, a viewer must not report one.
    #[must_use]
    pub const fn role_is_consistent(&self) -> bool {
        matches!(
            (self.role, &self.capability),
            (PeerRole::Viewer, None) | (PeerRole::Worker, Some(_))
        )
    }
}

/// Which compute backend a worker is rendering on, reported in [`RenderCapability`].
/// Variant order is wire-load-bearing; append only.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Backend {
    /// The CPU tracer on `threads` threads.
    Cpu {
        /// Worker threads the CPU tracer uses.
        threads: u32,
    },
    /// The GPU megakernel on the named adapter.
    Gpu {
        /// The adapter's human-readable label.
        adapter: String,
    },
    /// A coordinator fanning requests out over joined workers (v14). To a viewer it
    /// looks like one fast worker; these counts are for its "served by" line.
    Coordinator {
        /// Joined workers currently registered.
        workers: u32,
        /// Total CPU tracer threads across the coordinator's own lane and its workers.
        threads: u32,
        /// Total GPU adapters across the coordinator's own lane and its workers.
        gpus: u32,
    },
}

/// The render half of [`Welcome`], present only when this worker was built with (and
/// has active) render capacity -- see [`Welcome::render`]. Also what a joining worker
/// reports in [`Hello::capability`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RenderCapability {
    /// Which engine serves requests.
    pub backend: Backend,
    /// The largest `width * height` this worker is willing to render in one `RENDER`
    /// request.
    pub max_pixels: u32,
    /// This worker's cadence FLOOR, in milliseconds -- the fastest `StreamConfig::cadence_ms`
    /// it can usefully attempt. Purely advisory: a client may request a smaller
    /// `cadence_ms` anyway (rate-limited naturally by delta coalescing under
    /// backpressure), but a viewer UI can use this to grey out unreachable values.
    pub min_cadence_ms: u32,
    /// v14: whether this server renders scenes lit by an HDR panorama
    /// (`SceneEnvironment::Hdr`), fetching the map's bytes with `NEED_ASSET` when it lacks
    /// them. A viewer sends an HDR scene only to a server advertising `true`; anything
    /// else keeps HDR scenes local.
    pub hdr: bool,
}

/// What a coordinator hands a joining worker in [`Welcome::registration`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkerRegistration {
    /// The coordinator-assigned id for this worker connection, unique per coordinator
    /// process lifetime; for logs and diagnostics.
    pub worker_id: u32,
}

/// `<- WELCOME`: a worker's reply to `HELLO`, identifying itself and honestly
/// advertising what it can actually do -- see the module doc comment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Welcome {
    /// See [`Hello::protocol_version`]. Must stay the first field (module doc).
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
    /// `Some` only in a coordinator's reply to a [`PeerRole::Worker`] Hello; `None` for
    /// every viewer.
    pub registration: Option<WorkerRegistration>,
    /// The payload encoding this server negotiated for the connection
    /// (`super::encoding::negotiate`). An upper bound: each FRAME/PREVIEW header still
    /// names its own encoding, which may be `Raw` for an incompressible payload.
    pub payload_encoding: PayloadEncoding,
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

    fn round_trip<T>(msg: &T) -> T
    where
        T: Serialize + serde::de::DeserializeOwned,
    {
        let mut buf = Vec::new();
        write_message(&mut buf, msg).unwrap();
        let mut cursor = std::io::Cursor::new(buf);
        read_message(&mut cursor).unwrap()
    }

    fn cpu_capability() -> RenderCapability {
        RenderCapability {
            backend: Backend::Cpu { threads: 16 },
            max_pixels: 8_294_400,
            min_cadence_ms: 100,
            hdr: true,
        }
    }

    #[test]
    fn hello_round_trips_for_both_roles() {
        let viewer = Hello::viewer(PROTOCOL_VERSION, [1, 2, 3, 4, 5, 6, 7, 8], [8; 8]);
        assert_eq!(round_trip(&viewer), viewer);
        assert!(viewer.role_is_consistent());

        let worker = Hello {
            role: PeerRole::Worker,
            capability: Some(cpu_capability()),
            accept_encodings: vec![PayloadEncoding::ShuffleLz4, PayloadEncoding::Raw],
            ..viewer
        };
        assert_eq!(round_trip(&worker), worker);
        assert!(worker.role_is_consistent());
    }

    #[test]
    fn role_and_capability_must_agree() {
        let mut hello = Hello::viewer(PROTOCOL_VERSION, [1; 8], [2; 8]);
        hello.capability = Some(cpu_capability());
        assert!(
            !hello.role_is_consistent(),
            "a viewer reporting a capability"
        );
        hello.role = PeerRole::Worker;
        hello.capability = None;
        assert!(!hello.role_is_consistent(), "a worker without a capability");
    }

    #[test]
    fn welcome_round_trips_every_backend_variant_and_registration() {
        for backend in [
            Backend::Cpu { threads: 16 },
            Backend::Gpu {
                adapter: "RTX 4090".to_string(),
            },
            Backend::Coordinator {
                workers: 3,
                threads: 48,
                gpus: 2,
            },
        ] {
            for (registration, hdr) in [
                (None, false),
                (Some(WorkerRegistration { worker_id: 42 }), true),
            ] {
                let welcome = Welcome {
                    protocol_version: PROTOCOL_VERSION,
                    build_hash: [9; 8],
                    source_hash: [10; 8],
                    render: Some(RenderCapability {
                        backend: backend.clone(),
                        max_pixels: 8_294_400,
                        min_cadence_ms: 100,
                        hdr,
                    }),
                    library: true,
                    tilt_curves: true,
                    registration,
                    payload_encoding: PayloadEncoding::DEFAULT_ZSTD,
                };
                assert_eq!(round_trip(&welcome), welcome);
            }
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
            registration: None,
            payload_encoding: PayloadEncoding::Raw,
        };
        let decoded = round_trip(&welcome);
        assert_eq!(welcome, decoded);
        assert!(decoded.render.is_none());
        assert!(decoded.library);
        assert!(!decoded.tilt_curves);
    }

    /// Pins the `Backend`/`PeerRole` discriminants: `Coordinator` is appended after the
    /// two v13 variants, so a v13 `Cpu`/`Gpu` keeps its index.
    #[test]
    fn appended_enum_variants_keep_the_older_discriminants() {
        assert_eq!(
            postcard::to_allocvec(&Backend::Cpu { threads: 1 }).unwrap()[0],
            0
        );
        assert_eq!(
            postcard::to_allocvec(&Backend::Gpu {
                adapter: String::new()
            })
            .unwrap()[0],
            1
        );
        let coordinator = Backend::Coordinator {
            workers: 1,
            threads: 1,
            gpus: 0,
        };
        assert_eq!(postcard::to_allocvec(&coordinator).unwrap()[0], 2);
        assert_eq!(postcard::to_allocvec(&PeerRole::Viewer).unwrap(), [0]);
        assert_eq!(postcard::to_allocvec(&PeerRole::Worker).unwrap(), [1]);
    }
}
