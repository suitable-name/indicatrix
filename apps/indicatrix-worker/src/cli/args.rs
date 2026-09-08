//! The parsed-argument types [`super::parse::parse`] produces -- one struct per
//! subcommand, plus the [`Command`] enum that wraps them.

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderArgs {
    pub scene: PathBuf,
    pub out: PathBuf,
    pub width: u32,
    pub height: u32,
    pub samples: u32,
    /// `0` means "let the OS decide" -- see `render_core::effective_thread_count`.
    /// Governs only the CPU tracer -- see `USAGE_RENDER`'s `--threads` entry.
    pub threads: usize,
    /// `--only-gpu`/`--only-cpu` (default `Hybrid`) -- see [`ComputeMode`]'s own doc
    /// comment. See `USAGE_RENDER`'s entries and
    /// `indicatrix::renderer::gpu_backend::GpuBackend::disabled`/`::acquire`.
    pub compute_mode: ComputeMode,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServeArgs {
    pub bind: String,
    /// `0` means "let the OS decide" -- see `render_core::effective_thread_count`.
    /// Governs only the CPU tracer -- see `USAGE_SERVE`'s `--threads` entry.
    pub threads: usize,
    pub allow_remote: bool,
    /// CA certificate path. Required by `serve::run` unless `insecure_no_tls` is set;
    /// left optional here because parsing doesn't validate that dependency.
    pub ca: Option<PathBuf>,
    pub cert: Option<PathBuf>,
    pub key: Option<PathBuf>,
    /// Defaults to `pki::default_allowlist_path(ca)` (allowlist.txt next to `--ca`)
    /// when `None` -- see `serve::run`.
    pub allowlist: Option<PathBuf>,
    pub trust_any_client_cert: bool,
    pub insecure_no_tls: bool,
    /// `--only-gpu`/`--only-cpu` (default `Hybrid`) -- see [`ComputeMode`]. `OnlyCpu`
    /// forces `Backend::Cpu` for every request even with a usable GPU adapter.
    pub compute_mode: ComputeMode,
    /// `--enroll-bind`: address for the token-based enrollment listener. Defaults to
    /// the same host as `bind`, one port up (`crate::enroll::EnrollConfig::build`).
    /// Unused when `insecure_no_tls` is set.
    pub enroll_bind: Option<String>,
    /// `--no-enroll`: don't start the enrollment listener at all.
    pub no_enroll: bool,
    /// `--db <path>`: the design-library database this `serve` instance serves (see
    /// `crate::serve::library`). `None` keeps the existing default -- a
    /// `facet_diagrams.sqlite` resolved relative to the process's working directory.
    pub db: Option<PathBuf>,
    /// `--max-connections <n>`: the most connections `serve::run` handles at once
    /// (default 64, see `serve::ConnectionLimiter`) -- one cap shared by the one
    /// listener regardless of transport (TLS or `--insecure-no-tls`) or build mode
    /// (library-only or `worker`). A connection past the cap is still accepted and told
    /// so with a definitive `<- ERROR` reply, not left to hang.
    pub max_connections: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CertInitArgs {
    pub dir: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CertIssueServerArgs {
    pub dir: PathBuf,
    pub hosts: Vec<String>,
    pub ips: Vec<IpAddr>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CertIssueClientArgs {
    pub dir: PathBuf,
    pub name: String,
    pub out: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CertIssueTokenArgs {
    pub ca: PathBuf,
    pub admin_addr: String,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CertClaimArgs {
    pub token: String,
    pub addr: String,
    pub out: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    Render(RenderArgs),
    Serve(ServeArgs),
    CertInit(CertInitArgs),
    CertIssueServer(CertIssueServerArgs),
    CertIssueClient(CertIssueClientArgs),
    CertIssueToken(CertIssueTokenArgs),
    CertClaim(CertClaimArgs),
    Help(HelpTopic),
}

/// Which `--help` page a command line resolves to -- see
/// [`super::parse::HelpTopic::from_argv`] for the resolution rule and
/// [`super::USAGE_ROOT`] and its siblings for the actual page text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HelpTopic {
    /// `indicatrix-worker --help` / `indicatrix-worker -h` / no arguments at all: the
    /// three subcommands with one-line descriptions.
    Root,
    /// `indicatrix-worker render --help`: render's own usage line and flags only.
    Render,
    /// `indicatrix-worker serve --help`: serve's own usage lines and flags only.
    Serve,
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
