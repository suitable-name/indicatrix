//! The parsed-argument types [`super::parse::parse`] produces -- one struct per
//! subcommand, plus the [`Command`] enum that wraps them.

use indicatrix_net::messages::{PeerRole, adaptive::PayloadChoice};
use std::{net::IpAddr, path::PathBuf};

/// Which engine(s) trace a request. See `USAGE_RENDER`/`USAGE_SERVE`'s
/// `--only-gpu`/`--only-cpu` entries for the selecting flags; passing both is a parse
/// error, not silent last-wins ([`super::parse::parse`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ComputeMode {
    /// CPU and GPU trace concurrently, disjoint sample sub-ranges, whenever
    /// `stream_emit::tracer::run_tracer` judges the split worthwhile (see
    /// `render_core::hybrid::calibrate`; declining falls back to GPU-only, logged once
    /// at `info`). The default. Only `run_tracer`'s batch loop calibrates, so outside
    /// it this is indistinguishable from [`OnlyGpu`](Self::OnlyGpu).
    #[default]
    Hybrid,
    /// GPU only -- never splits work onto the CPU tracer (skips `calibrate` entirely).
    /// Still falls back to the CPU tracer for a whole request/sub-batch the GPU itself
    /// declines (no adapter, unsupported material); this turns off only the
    /// concurrent split. Rejected at parse time without the `gpu` feature.
    OnlyGpu,
    /// CPU only -- forces [`indicatrix::renderer::gpu_backend::GpuBackend::disabled`]
    /// even on a `gpu`-feature build with a usable adapter. Always valid, on any build.
    OnlyCpu,
}

/// Arguments of `indicatrix-worker render`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderArgs {
    /// Scene file to render.
    pub scene: PathBuf,
    /// Output image path.
    pub out: PathBuf,
    /// Image width in pixels.
    pub width: u32,
    /// Image height in pixels.
    pub height: u32,
    /// Samples per pixel.
    pub samples: u32,
    /// `0` means "let the OS decide" -- see `render_core::effective_thread_count`.
    /// Governs only the CPU tracer -- see `USAGE_RENDER`'s `--threads` entry.
    pub threads: usize,
    /// `--only-gpu`/`--only-cpu` (default `Hybrid`) -- see [`ComputeMode`]'s own doc
    /// comment. See `USAGE_RENDER`'s entries and
    /// `indicatrix::renderer::gpu_backend::GpuBackend::disabled`/`::acquire`.
    pub compute_mode: ComputeMode,
}

/// Arguments of `indicatrix-worker serve`.
#[derive(Debug, Clone, PartialEq)]
pub struct ServeArgs {
    /// Listen address (`host:port`).
    pub bind: String,
    /// `0` means "let the OS decide" -- see `render_core::effective_thread_count`.
    /// Governs only the CPU tracer -- see `USAGE_SERVE`'s `--threads` entry.
    pub threads: usize,
    /// Whether to bind a non-loopback address.
    pub allow_remote: bool,
    /// CA certificate path. Required by `serve::run` unless `insecure_no_tls` is set;
    /// left optional here because parsing doesn't validate that dependency.
    pub ca: Option<PathBuf>,
    /// Server certificate path.
    pub cert: Option<PathBuf>,
    /// Server private key path.
    pub key: Option<PathBuf>,
    /// The VIEWER allowlist. Defaults to `pki::default_viewer_allowlist_path(ca)`
    /// (`allowlist-viewers.txt` next to `--ca`, or a pre-role `allowlist.txt` while only
    /// that exists) when `None` -- see `serve::run`.
    pub allowlist: Option<PathBuf>,
    /// Accept any client certificate that chains to the CA, skipping the allowlist.
    pub trust_any_client_cert: bool,
    /// Serve plain TCP without TLS.
    pub insecure_no_tls: bool,
    /// `--only-gpu`/`--only-cpu` (default `Hybrid`) -- see [`ComputeMode`]. `OnlyCpu`
    /// forces `Backend::Cpu` for every request even with a usable GPU adapter. Only
    /// meaningful with [`Self::render`]; either flag implies it.
    pub compute_mode: ComputeMode,
    /// `--render` (implied by `--only-gpu`/`--only-cpu`): this coordinator renders with
    /// its own CPU/GPU lane. Off by default since coordinator mode: without it `serve`
    /// serves the library and accepts joining workers, but never traces a sample itself
    /// and never acquires the GPU.
    pub render: bool,
    /// `--enroll-bind`: address for the VIEWER token-based enrollment listener. Defaults
    /// to the same host as `bind`, one port up (7879 for the default 7878). Unused when
    /// `insecure_no_tls` is set.
    pub enroll_bind: Option<String>,
    /// `--no-enroll`: don't start either enrollment listener (viewer or worker).
    pub no_enroll: bool,
    /// `--worker-bind`: address of the worker port joining workers dial. Defaults to the
    /// same host as `bind`, two ports up (7880 for 7878).
    pub worker_bind: Option<String>,
    /// `--worker-enroll-bind`: address for the WORKER enrollment listener. Defaults to
    /// the worker port's host, one port up (7881 for 7880).
    pub worker_enroll_bind: Option<String>,
    /// `--no-workers`: don't open the worker port (nor its enrollment listener).
    pub no_workers: bool,
    /// `--worker-allowlist`: fingerprints of trusted WORKER certificates. Defaults to
    /// `allowlist-workers.txt` next to `--ca`.
    pub worker_allowlist: Option<PathBuf>,
    /// `--db <path>`: the design-library database this `serve` instance serves (see
    /// `crate::serve::library`). `None` keeps the existing default -- a
    /// `facet_diagrams.sqlite` resolved relative to the process's working directory.
    pub db: Option<PathBuf>,
    /// `--max-connections <n>`: the most connections `serve::run` handles at once
    /// (default 64, see `serve::ConnectionLimiter`), counted separately for the viewer
    /// port and for joined workers on the worker port -- each listener gets its own cap
    /// of `n`, regardless of transport (TLS or `--insecure-no-tls`) or build mode
    /// (library-only or `worker`). A connection past its cap is still accepted and told
    /// so with a definitive `<- ERROR` reply, not left to hang.
    pub max_connections: usize,
    /// `--max-preauth-per-ip <n>` (default [`super::DEFAULT_MAX_PREAUTH_PER_IP`], at
    /// least 1): the most viewer-port connections one source IP address may have open
    /// that have not finished authenticating. A connection stops counting once its TLS
    /// handshake and allowlist check succeed (for `--insecure-no-tls`, once its
    /// `--max-connections` slot is decided); a connection over the cap is closed at once.
    pub max_preauth_per_ip: usize,
    /// `--interactive-workers <n|all>` (advanced, default
    /// [`super::ALL_INTERACTIVE_WORKERS`], i.e. `all`): how many of the fastest idle
    /// joined workers an `Interactive` (live-view) request takes besides the own lane;
    /// `all` is every idle eligible worker. `0` keeps the live view on the own lane
    /// alone; a coordinator without `--render` then uses the single fastest worker.
    pub interactive_workers: u32,
    /// `--pin-interactive-worker <label>` (advanced): the joined worker (by its
    /// certificate label, the `<label>` of `worker:<label>`) that `Interactive` requests
    /// taking workers use first while it is connected, idle and eligible; otherwise the
    /// fastest-by-rate rule applies. `None` (default): no pin.
    pub pin_interactive_worker: Option<String>,
    /// `--max-job-memory-mib <n>` (default [`super::DEFAULT_MAX_JOB_MEMORY_MIB`]): the
    /// in-flight buffer budget of multi-lane coordinator jobs (each charged
    /// width x height x 48 bytes, plus width x height x 36 bytes per lane and its HDR
    /// map and viewer contribution bytes); a job that would exceed it even with a single
    /// lane is refused.
    pub max_job_memory_mib: u32,
    /// `--whole-image-secs <secs>` (default [`super::DEFAULT_WHOLE_IMAGE_SECS`]): a
    /// `Batch` request whose estimated render time on the fastest eligible joined worker
    /// is below this goes to one lane as a whole picture instead of being split.
    pub whole_image_secs: f64,
    /// `--whole-image-pixel-samples <n>` (default
    /// [`super::DEFAULT_WHOLE_IMAGE_PIXEL_SAMPLES`]): the same decision, by `width x height
    /// x samples`, while no joined worker has a measured rate yet.
    pub whole_image_pixel_samples: u64,
    /// `--jobs-per-viewer <n>` (default [`super::DEFAULT_JOBS_PER_VIEWER`], at least 1):
    /// the most whole-image jobs one viewer certificate has running at once.
    pub jobs_per_viewer: u32,
    /// `--payload-encoding auto|raw|lz4|zstd[:LEVEL]` (default `auto`): how this
    /// coordinator compresses the `FRAME`/`PREVIEW`/`DISPLAY_FRAME` payloads it sends each
    /// viewer. `auto` follows each connection's measured speed (loopback peers get raw);
    /// the others pin one encoding for every link.
    pub payload_encoding: PayloadChoice,
}

/// Arguments of `cert init`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CertInitArgs {
    /// Directory the CA is created in.
    pub dir: PathBuf,
}

/// Arguments of `cert issue-server`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CertIssueServerArgs {
    /// Directory holding the CA.
    pub dir: PathBuf,
    /// DNS names for the certificate.
    pub hosts: Vec<String>,
    /// IP addresses for the certificate.
    pub ips: Vec<IpAddr>,
}

/// Arguments of `cert issue-client`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CertIssueClientArgs {
    /// Directory holding the CA.
    pub dir: PathBuf,
    /// Client name placed in the certificate.
    pub name: String,
    /// Output bundle path.
    pub out: PathBuf,
    /// `--role viewer|worker` (default viewer) -- see `crate::pki::role`.
    pub role: PeerRole,
}

/// Arguments of `cert issue-token`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CertIssueTokenArgs {
    /// CA directory or file used to sign.
    pub ca: PathBuf,
    /// Enrollment listener address.
    pub admin_addr: String,
    /// Client name the token enrolls.
    pub name: String,
    /// `--role viewer|worker` (default viewer): which enrollment listener `admin_addr`
    /// must be -- the viewer one (7879) or the worker one (7881). A mismatch is refused
    /// by the listener, never silently minted in the wrong role.
    pub role: PeerRole,
}

/// `indicatrix-worker join <coordinator-host:port> ...` (worker feature): dial a
/// coordinator's worker port and serve its render requests.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JoinArgs {
    /// The coordinator's WORKER port (`host:port`, default port 7880), given
    /// positionally or as `--coordinator`.
    pub coordinator: String,
    /// Where the worker certificate bundle (`ca.pem`, `client.pem`, `client.key`) lives
    /// -- or, with `--token`, where the claimed bundle is written first. Default
    /// `worker-cert` in the working directory.
    pub cert_dir: PathBuf,
    /// `--slots K` (default 2): parallel connections, each serving one request stream.
    /// A joined worker's own chunk transfer/decode gap otherwise leaves it idle between
    /// chunks on a single slot (see `DEFAULT_JOIN_SLOTS`'s own doc comment); 2 keeps a
    /// second chunk in flight to cover it.
    pub slots: usize,
    /// `--threads` for the CPU tracer; `0` = all cores.
    pub threads: usize,
    /// `--only-gpu`/`--only-cpu` (default hybrid).
    pub compute_mode: ComputeMode,
    /// `--token GW1-...`: claim a WORKER enrollment token first.
    pub token: Option<String>,
    /// `--enroll-addr`: the coordinator's worker enrollment listener. Defaults to the
    /// coordinator host, one port above `coordinator` (7881 for 7880).
    pub enroll_addr: Option<String>,
    /// `--payload-encoding auto|raw|lz4|zstd[:LEVEL]` (default `auto`): how this worker
    /// compresses the frames it sends the coordinator (see [`ServeArgs::payload_encoding`]).
    pub payload_encoding: PayloadChoice,
}

/// Arguments of `cert claim`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CertClaimArgs {
    /// Enrollment token to redeem.
    pub token: String,
    /// Enrollment listener address.
    pub addr: String,
    /// Where the claimed bundle is written.
    pub out: PathBuf,
}

/// A parsed command line.
#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    /// Render a scene to an image file.
    Render(RenderArgs),
    /// Boxed: by far the largest variant (`clippy::large_enum_variant`).
    Serve(Box<ServeArgs>),
    /// Join a coordinator as a render worker.
    Join(JoinArgs),
    /// Create a certificate authority.
    CertInit(CertInitArgs),
    /// Issue a server certificate.
    CertIssueServer(CertIssueServerArgs),
    /// Issue a client certificate bundle.
    CertIssueClient(CertIssueClientArgs),
    /// Mint an enrollment token.
    CertIssueToken(CertIssueTokenArgs),
    /// Redeem an enrollment token.
    CertClaim(CertClaimArgs),
    /// Print a help page.
    Help(HelpTopic),
}

/// Which `--help` page a command line resolves to -- see
/// [`super::parse::HelpTopic::from_argv`] for the resolution rule and
/// [`super::USAGE_ROOT`] and its siblings for the actual page text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HelpTopic {
    /// `indicatrix-worker --help` / `indicatrix-worker -h` / no arguments at all: the
    /// four subcommands (`render`, `serve`, `join`, `cert`) with one-line descriptions.
    Root,
    /// `indicatrix-worker render --help`: render's own usage line and flags only.
    Render,
    /// `indicatrix-worker serve --help`: serve's own usage lines and flags only.
    Serve,
    /// `indicatrix-worker join --help`.
    Join,
    /// `indicatrix-worker cert --help`: the CA overview plus the five sub-subcommands
    /// with one-line descriptions.
    Cert,
    /// `indicatrix-worker cert init --help`.
    CertInit,
    /// `indicatrix-worker cert issue-server --help`.
    CertIssueServer,
    /// `indicatrix-worker cert issue-client --help`.
    CertIssueClient,
    /// `indicatrix-worker cert issue-token --help`.
    CertIssueToken,
    /// `indicatrix-worker cert claim --help`.
    CertClaim,
}
