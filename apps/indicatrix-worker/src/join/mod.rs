//! The `join` subcommand (`worker` builds).
//!
//! A render worker that dials OUT to a coordinator's worker port and serves its
//! requests -- no inbound port needed on this machine, so NAT and cloud VMs work.
//!
//! The worker is the TLS/TCP CLIENT but the PROTOCOL SERVER: after TLS and a worker
//! `HELLO` (carrying this machine's `RenderCapability`), the coordinator sends
//! `ClientMessage`s and this side answers `StreamEvent`s through the very same request
//! loop a viewer connection uses (`crate::serve::serve_requests`), `PING` → `PONG`
//! included. `--slots K` opens K such connections (one request stream each); every slot
//! reconnects forever with jittered exponential backoff (1 s doubling to 60 s, reset
//! after every successful registration).
//!
//! `--token GW1-...` first claims a WORKER enrollment token over the coordinator's
//! worker enrollment listener (default: worker port + 1, 7881) and stores the bundle in
//! `--cert-dir`.

mod session;

pub use session::{
    IdleDeadlineStream, JOIN_IDLE_TIMEOUT, JoinError, JoinTarget, Session, WorkerSetup,
    bundle_role, join_once,
};

use crate::cli::{ComputeMode, JoinArgs};
use indicatrix::renderer::gpu_backend::GpuBackend;
use indicatrix_net::messages::PeerRole;
use std::{
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Duration,
};

/// The first reconnect delay (before jitter).
pub const BACKOFF_MIN: Duration = Duration::from_secs(1);
/// The longest reconnect delay (before jitter).
pub const BACKOFF_MAX: Duration = Duration::from_secs(60);

/// Runs the `join` subcommand: optional token claim, bundle and role check, then
/// `args.slots` reconnecting slots, forever.
///
/// # Errors
///
/// A human-readable message for anything that can't get better by retrying: a failed
/// token claim, an unreadable bundle, or a bundle that is not a WORKER certificate.
pub fn run(args: &JoinArgs) -> Result<(), String> {
    if let Some(token) = &args.token {
        let enroll_addr = match &args.enroll_addr {
            Some(addr) => addr.clone(),
            None => default_enroll_addr(&args.coordinator)?,
        };
        let bundle = indicatrix_net::enroll::claim(token, &enroll_addr)
            .map_err(|e| format!("claiming the worker token at {enroll_addr}: {e}"))?;
        crate::enroll_client::write_bundle(&args.cert_dir, &bundle)?;
        tracing::info!(
            "indicatrix-worker join: claimed a worker certificate into {}",
            args.cert_dir.display()
        );
    }
    check_worker_bundle(&args.cert_dir)?;
    let target = Arc::new(JoinTarget::from_bundle(&args.coordinator, &args.cert_dir)?);

    // Acquired once; a device lost later is re-acquired by the backend itself (cool-down
    // and hourly attempt budget in `indicatrix::renderer::gpu_backend`), and every
    // reconnect advertises the backend's state at that moment
    // (`WorkerSetup::current_capability`).
    let gpu = Arc::new(if args.compute_mode == ComputeMode::OnlyCpu {
        GpuBackend::disabled()
    } else {
        GpuBackend::acquire()
    });
    // Makes the hybrid CPU/GPU split decision once, before the coordinator's first real
    // chunk, rather than paying its 3-sample probe on whichever chunk happens to arrive
    // first. Only meaningful with a real adapter and the hybrid path (`OnlyGpu`/
    // `OnlyCpu` never calibrate a split at all -- see `render_core::hybrid::job_key`'s
    // doc comment).
    if args.compute_mode == ComputeMode::Hybrid && gpu.adapter_label().is_some() {
        crate::render_core::hybrid::calibrate_now(
            &gpu,
            &probe_scene(),
            args.threads,
            args.compute_mode,
        );
    }
    // HDR maps are cached next to the certificate bundle (or where
    // INDICATRIX_ASSET_CACHE_DIR says); without a cache this worker takes no HDR jobs.
    let assets = crate::assets::open_configured(&args.cert_dir);
    // The start-up baseline: the HELLO recomputes its backend from `gpu` per connection.
    let capability = indicatrix_net::messages::RenderCapability {
        hdr: assets.is_some(),
        ..crate::serve::local_render_capability(&gpu, args.threads)
    };
    tracing::info!(
        "indicatrix-worker join: joining {} with {} slot(s) as {:?} (HDR environments: {})",
        args.coordinator,
        args.slots,
        capability.backend,
        if capability.hdr { "yes" } else { "no" }
    );
    let setup = Arc::new(WorkerSetup {
        gpu,
        threads: args.threads,
        compute_mode: args.compute_mode,
        capability,
        assets,
        payload: args.payload_encoding,
    });
    let stop = Arc::new(AtomicBool::new(false));
    let slots: Vec<_> = (0..args.slots)
        .map(|slot| {
            let (target, setup, stop) =
                (Arc::clone(&target), Arc::clone(&setup), Arc::clone(&stop));
            thread::spawn(move || run_slot(slot, &target, &setup, &stop))
        })
        .collect();
    for slot in slots {
        if slot.join().is_err() {
            tracing::error!("indicatrix-worker join: a slot thread panicked");
        }
    }
    Ok(())
}

/// A representative scene for [`crate::render_core::hybrid::calibrate_now`]'s start-up
/// probe: a real, traceable scene (so the probe measures genuine per-dispatch overhead,
/// not just call overhead) at a typical interactive/live-view resolution. Only its
/// resolution matters to the calibration decision (`render_core::hybrid::job_key`
/// buckets to the enclosing power of two), so the exact material/geometry/lighting
/// below are arbitrary.
fn probe_scene() -> indicatrix_net::SceneState {
    use indicatrix::{
        geometry::cuts::StandardGemCuts,
        optics::{materials::GemMaterial, raytracer::LightingPreset},
    };
    indicatrix_net::SceneState {
        width: 512,
        height: 512,
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
    }
}

/// Refuses a bundle whose certificate is not a WORKER certificate, with the fix.
fn check_worker_bundle(cert_dir: &Path) -> Result<(), String> {
    match bundle_role(cert_dir).map_err(|e| format!("--cert-dir {}: {e}", cert_dir.display()))? {
        PeerRole::Worker => Ok(()),
        PeerRole::Viewer => Err(format!(
            "--cert-dir {}: this is a VIEWER certificate -- `join` needs a worker certificate (`cert issue-client \
             --role worker` on the coordinator, or `join --token` with a `cert issue-token --role worker` token)",
            cert_dir.display()
        )),
    }
}

/// The worker enrollment listener's default address: the coordinator host, one port
/// above its worker port (7881 for 7880).
///
/// # Errors
///
/// A human-readable message if `coordinator` isn't `host:port`.
pub fn default_enroll_addr(coordinator: &str) -> Result<String, String> {
    let (host, port) = coordinator
        .rsplit_once(':')
        .ok_or_else(|| format!("coordinator address {coordinator:?} must be host:port"))?;
    let port: u16 = port
        .parse()
        .map_err(|_| format!("coordinator address {coordinator:?}: bad port"))?;
    let enroll_port = port
        .checked_add(1)
        .ok_or_else(|| "pass --enroll-addr explicitly (the worker port is 65535)".to_string())?;
    Ok(format!("{host}:{enroll_port}"))
}

/// One slot: join, serve until the connection ends, back off, repeat -- until `stop`.
pub fn run_slot(slot: usize, target: &JoinTarget, setup: &WorkerSetup, stop: &AtomicBool) {
    let mut backoff = Backoff::new();
    while !stop.load(Ordering::Relaxed) {
        match join_once(target, setup) {
            Ok(session) => {
                backoff.reset();
                match session.ended {
                    Ok(()) => tracing::info!(
                        "indicatrix-worker join: slot {slot}: the coordinator closed worker #{}'s connection",
                        session.worker_id
                    ),
                    Err(e) => tracing::warn!(
                        "indicatrix-worker join: slot {slot}: worker #{}'s connection ended: {e}",
                        session.worker_id
                    ),
                }
            }
            Err(e) => tracing::warn!(
                "indicatrix-worker join: slot {slot}: {} -- {e}",
                target.addr
            ),
        }
        let delay = backoff.next_delay();
        tracing::info!("indicatrix-worker join: slot {slot}: reconnecting in {delay:.1?}");
        sleep_unless_stopped(delay, stop);
    }
}

/// Sleeps `delay` in short steps, returning early once `stop` is set.
fn sleep_unless_stopped(delay: Duration, stop: &AtomicBool) {
    let step = Duration::from_millis(200);
    let mut left = delay;
    while !left.is_zero() && !stop.load(Ordering::Relaxed) {
        let now = left.min(step);
        thread::sleep(now);
        left -= now;
    }
}

/// Jittered exponential backoff: [`BACKOFF_MIN`] doubling to [`BACKOFF_MAX`], each delay
/// scaled by a random factor in `[0.5, 1.0)` so many workers reconnecting after a
/// coordinator restart don't arrive in lockstep.
#[derive(Debug)]
pub struct Backoff {
    next: Duration,
}

impl Backoff {
    /// Starts at [`BACKOFF_MIN`].
    #[must_use]
    pub const fn new() -> Self {
        Self { next: BACKOFF_MIN }
    }

    /// Back to [`BACKOFF_MIN`] (after a successful registration).
    pub const fn reset(&mut self) {
        self.next = BACKOFF_MIN;
    }

    /// The next delay (jittered), doubling the base for the one after.
    pub fn next_delay(&mut self) -> Duration {
        let base = self.next;
        self.next = (self.next * 2).min(BACKOFF_MAX);
        base.mul_f64(random_unit().mul_add(0.5, 0.5))
    }
}

impl Default for Backoff {
    fn default() -> Self {
        Self::new()
    }
}

/// A random number in `[0, 1)` from the system CSPRNG (`ring`, already this crate's
/// randomness source); `0.5` if it fails.
fn random_unit() -> f64 {
    use ring::rand::SecureRandom;
    let mut bytes = [0_u8; 4];
    if ring::rand::SystemRandom::new().fill(&mut bytes).is_err() {
        return 0.5;
    }
    f64::from(u32::from_le_bytes(bytes)) / (f64::from(u32::MAX) + 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_doubles_to_the_cap_with_jitter_and_resets() {
        let mut backoff = Backoff::new();
        let mut bases = Vec::new();
        for _ in 0..10 {
            let base = backoff.next;
            let delay = backoff.next_delay();
            assert!(
                delay >= base / 2 && delay <= base,
                "{delay:?} vs base {base:?}"
            );
            bases.push(base.as_secs());
        }
        assert_eq!(bases, [1, 2, 4, 8, 16, 32, 60, 60, 60, 60]);
        backoff.reset();
        assert_eq!(backoff.next, BACKOFF_MIN);
    }

    #[test]
    fn the_enroll_address_defaults_to_one_port_above_the_worker_port() {
        assert_eq!(
            default_enroll_addr("coord.lan:7880").unwrap(),
            "coord.lan:7881"
        );
        assert_eq!(default_enroll_addr("[::1]:7880").unwrap(), "[::1]:7881");
        assert!(default_enroll_addr("coord.lan").is_err());
        assert!(default_enroll_addr("coord.lan:65535").is_err());
    }
}
