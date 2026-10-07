//! Shared test fixtures for `serve`'s tests: scenes, the throwaway database builder, the
//! `Read + Write` test doubles (`DuplexHalf`/`BackpressureDuplex`), and the
//! `StreamConfig`/`StreamEvent` helpers every topic file in this folder uses.

use crate::{serve::library::LibraryHandle, stream_emit::TimeoutRead};
use indicatrix::{
    geometry::cuts::StandardGemCuts,
    optics::{materials::GemMaterial, raytracer::LightingPreset},
};
use indicatrix_net::{SceneState, messages::StreamEvent};
use indicatrix_vault::db::sqlite::Database;
use std::io::{Cursor, Read, Write};

/// A unique path for a throwaway temp database, named after the process id and a
/// nanosecond timestamp so parallel tests never collide. Tests never touch
/// `facet_diagrams.sqlite`, only their own throwaway temp files.
fn unique_db_path() -> std::path::PathBuf {
    std::env::temp_dir().join(format!(
        "indicatrix-worker-serve-test-{}-{}.sqlite",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ))
}

/// Creates a fresh, empty, throwaway temp database file and returns its path (the
/// handle used to create it is already dropped, so a connection can open it itself).
pub(super) fn test_db_file() -> std::path::PathBuf {
    let path = unique_db_path();
    drop(Database::new(Some(path.to_str().unwrap())).unwrap());
    path
}

/// A [`LibraryHandle`] around a fresh, empty, throwaway temp database, just to satisfy
/// `handle_connection`'s signature -- none of these tests but `lazy_library`'s exercise
/// the library protocol itself.
pub(super) fn test_db() -> LibraryHandle {
    let path = unique_db_path();
    LibraryHandle::open(Database::new(Some(path.to_str().unwrap())).unwrap())
}

/// A unique, freshly created temp directory named after `label`, the process id and a
/// nanosecond timestamp, so parallel tests never collide.
pub fn unique_temp_dir(label: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "indicatrix-worker-test-{label}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

pub(super) fn tiny_scene() -> SceneState {
    SceneState {
        width: 4,
        height: 4,
        yaw: 0.4,
        pitch: 0.3,
        distance: 3.0,
        light_yaw: 0.85,
        light_pitch: 0.95,
        exposure: 1.0,
        max_bounces: 4,
        lighting_preset: LightingPreset::Daylight,
        material: GemMaterial::diamond(),
        planes: StandardGemCuts::standard_round_brilliant(),
        girdle_frosted: false,
        backdrop: 0.0,
        environment: indicatrix_net::scene::SceneEnvironment::Studio,
        surface_glare: 1.0,
        tools: Vec::new(),
        fluorescence: indicatrix::optics::fluorescence::Fluorescence::default(),
        head_shadow_deg: 16.0,
    }
}

/// [`tiny_scene`] with dispersion removed: a Cauchy model with `b = c = 0` at
/// diamond's `n_d` (2.417), so every spectral channel refracts along the same
/// direction and no chromatic-termination decision can sit on its
/// `DIRECTION_MATCH_COS_TOL` knife edge -- see
/// `super::render_roundtrip::hybrid_gpu_and_cpu_split_sums_to_the_same_result_as_tracing_the_range_directly`.
#[cfg(feature = "gpu")]
pub(super) fn tiny_scene_without_dispersion() -> SceneState {
    let mut scene = tiny_scene();
    scene.material.dispersion = indicatrix::optics::dispersion::DispersionModel::Cauchy {
        a: 2.417,
        b: 0.0,
        c: 0.0,
    };
    scene
}

/// A scene with enough per-sample work that a several-dozen-sample request takes long
/// enough for `run_stream`'s emitter to get multiple chances to poll/emit before the
/// tracer finishes -- unlike `tiny_scene`, which can finish before the emitter's first
/// loop iteration runs. Used by tests that need to observe more than one emission
/// deterministically.
pub(super) fn heavier_scene() -> SceneState {
    SceneState {
        width: 24,
        height: 24,
        yaw: 0.4,
        pitch: 0.3,
        distance: 3.0,
        light_yaw: 0.85,
        light_pitch: 0.95,
        exposure: 1.0,
        max_bounces: 6,
        lighting_preset: LightingPreset::Daylight,
        material: GemMaterial::diamond(),
        planes: StandardGemCuts::standard_round_brilliant(),
        girdle_frosted: false,
        backdrop: 0.0,
        environment: indicatrix_net::scene::SceneEnvironment::Studio,
        surface_glare: 1.0,
        tools: Vec::new(),
        fluorescence: indicatrix::optics::fluorescence::Fluorescence::default(),
        head_shadow_deg: 16.0,
    }
}

/// A `Read + Write` over two independent in-memory buffers, standing in for one end
/// of a duplex connection: writes go to `out`, reads come from `in_`. Lets
/// `handle_connection` be driven with hand-assembled request bytes, no networking.
///
/// `TimeoutRead`-aware, to exercise `stream_emit::poll_for_client_message`'s polling
/// loop: with no timeout set (the default), exhausting `in_` reports `Ok(0)` (EOF).
/// With a timeout set (as `run_stream`'s emitter does while streaming), exhausting
/// `in_` reports `WouldBlock` instead -- "nothing new yet", letting a test simulate
/// "connection still open, no `CANCEL` sent" by simply not writing one.
pub(super) struct DuplexHalf {
    in_: Cursor<Vec<u8>>,
    pub(super) out: Vec<u8>,
    timeout_active: bool,
}

impl DuplexHalf {
    pub(super) const fn new(input: Vec<u8>) -> Self {
        Self {
            in_: Cursor::new(input),
            out: Vec::new(),
            timeout_active: false,
        }
    }
}

impl Read for DuplexHalf {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.in_.read(buf)?;
        if n == 0 && self.timeout_active {
            return Err(std::io::Error::new(
                std::io::ErrorKind::WouldBlock,
                "DuplexHalf: no more scripted input (yet)",
            ));
        }
        Ok(n)
    }
}

impl Write for DuplexHalf {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.out.write(buf)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl TimeoutRead for DuplexHalf {
    fn set_read_timeout(&mut self, duration: Option<std::time::Duration>) -> std::io::Result<()> {
        self.timeout_active = duration.is_some();
        Ok(())
    }
}

/// A no-op: `DuplexHalf::write` goes to an unbounded `Vec` and can never actually
/// block. See [`BackpressureDuplex`] below for a double that genuinely models a peer
/// who stops draining.
impl crate::stream_emit::TimeoutWrite for DuplexHalf {
    fn set_write_timeout(&mut self, _duration: Option<std::time::Duration>) -> std::io::Result<()> {
        Ok(())
    }
}

/// A `Read + Write` double modeling a peer that stops draining: `write()` succeeds
/// while `out` stays within `capacity`, then returns [`std::io::ErrorKind::TimedOut`]
/// as if a write-timeout deadline had already elapsed, so a test doesn't need a real
/// multi-second sleep. Only enforced while a write timeout is set
/// (`write_timeout_active`): the `WELCOME` handshake write always succeeds regardless
/// of `capacity`, like a real unbounded-by-default socket.
///
/// [`DuplexHalf`]'s unbounded-`Vec` `Write` side can never model this backpressure, so
/// it can't reproduce the emitter blocking inside an unbounded `write()` against
/// real sockets. `BackpressureDuplex` closes that gap deterministically.
pub(super) struct BackpressureDuplex {
    in_: Cursor<Vec<u8>>,
    pub(super) out: Vec<u8>,
    capacity: usize,
    read_timeout_active: bool,
    write_timeout_active: bool,
    /// The `io::ErrorKind` a write past `capacity` reports once `write_timeout_active`.
    /// Defaults to `TimedOut` (a raw socket's kind) via [`Self::new`]; see
    /// [`Self::new_with_kind`] to simulate rustls' `WriteZero` instead (see
    /// `crate::stream_emit::is_stream_timeout`).
    timeout_kind: std::io::ErrorKind,
}

impl BackpressureDuplex {
    pub(super) const fn new(input: Vec<u8>, capacity: usize) -> Self {
        Self::new_with_kind(input, capacity, std::io::ErrorKind::TimedOut)
    }

    /// Like [`Self::new`], but a write past `capacity` reports `kind` instead of always
    /// `TimedOut` -- lets a test simulate rustls' `WriteZero`-on-timeout behavior too.
    pub(super) const fn new_with_kind(
        input: Vec<u8>,
        capacity: usize,
        kind: std::io::ErrorKind,
    ) -> Self {
        Self {
            in_: Cursor::new(input),
            out: Vec::new(),
            capacity,
            read_timeout_active: false,
            write_timeout_active: false,
            timeout_kind: kind,
        }
    }
}

impl Read for BackpressureDuplex {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.in_.read(buf)?;
        if n == 0 && self.read_timeout_active {
            return Err(std::io::Error::new(
                std::io::ErrorKind::WouldBlock,
                "BackpressureDuplex: no more scripted input (yet)",
            ));
        }
        Ok(n)
    }
}

impl Write for BackpressureDuplex {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if self.write_timeout_active && self.out.len() + buf.len() > self.capacity {
            return Err(std::io::Error::new(
                self.timeout_kind,
                "BackpressureDuplex: peer isn't draining -- write timed out",
            ));
        }
        self.out.write(buf)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl TimeoutRead for BackpressureDuplex {
    fn set_read_timeout(&mut self, duration: Option<std::time::Duration>) -> std::io::Result<()> {
        self.read_timeout_active = duration.is_some();
        Ok(())
    }
}

impl crate::stream_emit::TimeoutWrite for BackpressureDuplex {
    fn set_write_timeout(&mut self, duration: Option<std::time::Duration>) -> std::io::Result<()> {
        self.write_timeout_active = duration.is_some();
        Ok(())
    }
}

pub(super) const fn live_progressive(cadence_ms: u32) -> indicatrix_net::messages::StreamConfig {
    indicatrix_net::messages::StreamConfig {
        transfer_mode: indicatrix_net::messages::TransferMode::LiveProgressive,
        cadence_ms,
        preview: None,
    }
}

pub(super) const fn final_only(cadence_ms: u32) -> indicatrix_net::messages::StreamConfig {
    indicatrix_net::messages::StreamConfig {
        transfer_mode: indicatrix_net::messages::TransferMode::FinalOnly,
        cadence_ms,
        preview: None,
    }
}

/// Reads [`indicatrix_net::messages::StreamEvent`]s from `reader` (pairing each with
/// its raw payload, for `Frame`/`Preview`) until -- and including -- a `Done` or
/// `Error`, the two terminal variants for one `RENDER` reply.
pub(super) fn read_stream_until_done<R: Read>(
    reader: &mut R,
) -> Vec<(indicatrix_net::messages::StreamEvent, Option<Vec<u8>>)> {
    let mut events = Vec::new();
    loop {
        let (event, payload) = indicatrix_net::messages::read_stream_event(reader).unwrap();
        let terminal = matches!(
            event,
            indicatrix_net::messages::StreamEvent::Done(_)
                | indicatrix_net::messages::StreamEvent::Error(_)
        );
        events.push((event, payload));
        if terminal {
            break;
        }
    }
    events
}

/// The `request_id` carried by one `StreamEvent` -- every variant but `Error`
/// carries one; see `indicatrix_net::messages`' docs on why that's what makes a stale
/// reply mechanically identifiable.
pub(super) fn event_request_id(event: &StreamEvent) -> u32 {
    event
        .request_id()
        .unwrap_or_else(|| panic!("unexpected request-less StreamEvent: {event:?}"))
}
