//! Hand-rolled argument parsing -- no external CLI-parsing crate.
//!
//! The surface is small (four subcommands -- `render`, `serve`, `join` and `cert`, the
//! last with five sub-subcommands -- each a flat list of `--flag [value]` pairs) and
//! every value needs its own validation message anyway, so a dependency buys little.

mod args;
mod parse;
#[cfg(test)]
mod tests;

pub use args::{
    CertClaimArgs, CertInitArgs, CertIssueClientArgs, CertIssueServerArgs, CertIssueTokenArgs,
    Command, ComputeMode, HelpTopic, JoinArgs, RenderArgs, ServeArgs,
};
pub use parse::parse;

/// Default listen address of `serve`.
pub const DEFAULT_BIND: &str = "127.0.0.1:7878";

/// `serve --max-connections`'s default -- see [`ServeArgs::max_connections`]'s doc
/// comment and `serve::ConnectionLimiter`.
pub const DEFAULT_MAX_CONNECTIONS: usize = 64;

/// `serve --max-preauth-per-ip`'s default -- see [`ServeArgs::max_preauth_per_ip`].
pub const DEFAULT_MAX_PREAUTH_PER_IP: usize = 8;

/// `serve --max-job-memory-mib`'s default: 2 GiB of in-flight coordinator job buffers
/// (see [`ServeArgs::max_job_memory_mib`]).
pub const DEFAULT_MAX_JOB_MEMORY_MIB: u32 = 2048;

/// `serve --interactive-workers all` (and `serve`'s default): every idle eligible joined
/// worker takes part in a live-view request. Carried as the largest `u32` in
/// [`ServeArgs::interactive_workers`].
pub const ALL_INTERACTIVE_WORKERS: u32 = u32::MAX;

/// `serve --whole-image-secs`'s default: a `Batch` request estimated to render in under 2 s
/// on the fastest eligible joined worker is one picture on one lane.
pub const DEFAULT_WHOLE_IMAGE_SECS: f64 = 2.0;

/// `serve --whole-image-pixel-samples`'s default: 64 Mi pixel-samples (`width x height x
/// samples`), the size rule while no joined worker has a measured rate.
pub const DEFAULT_WHOLE_IMAGE_PIXEL_SAMPLES: u64 = 64 * 1024 * 1024;

/// `serve --jobs-per-viewer`'s default: eight whole-image jobs per viewer certificate at once.
pub const DEFAULT_JOBS_PER_VIEWER: u32 = 8;

/// How far above `--bind`'s port the worker port sits by default (7878 -> 7880, leaving
/// 7879 to the viewer enrollment listener). Its enrollment listener sits one further up
/// (7881).
pub const WORKER_PORT_OFFSET: u16 = 2;

/// `join --cert-dir`'s default, relative to the working directory.
pub const DEFAULT_JOIN_CERT_DIR: &str = "worker-cert";

/// `join --slots`'s default: two connections.
///
/// A second chunk is already in flight when the first one's transfer/decode gap would
/// otherwise leave this worker idling in the coordinator's `Epoch::claim` between
/// chunks -- measured as an underused fast remote worker (e.g. an A100) behind a much
/// slower coordinator lane. Was `1` before this was measured; bump further only for a
/// CPU-only machine juggling many small chunks (see `USAGE_JOIN`).
pub const DEFAULT_JOIN_SLOTS: usize = 2;

/// The most `join --slots` accepted -- a sanity bound, not a tuning knob.
pub const MAX_JOIN_SLOTS: usize = 64;

impl HelpTopic {
    /// The `--help` page text for this topic. `main.rs` prints exactly this and
    /// nothing else -- each page is meant to stand alone on a terminal screen.
    #[must_use]
    pub const fn usage(&self) -> &'static str {
        match self {
            Self::Root => USAGE_ROOT,
            Self::Render => USAGE_RENDER,
            Self::Serve => USAGE_SERVE,
            Self::Join => USAGE_JOIN,
            Self::Cert => USAGE_CERT,
            Self::CertInit => USAGE_CERT_INIT,
            Self::CertIssueServer => USAGE_CERT_ISSUE_SERVER,
            Self::CertIssueClient => USAGE_CERT_ISSUE_CLIENT,
            Self::CertIssueToken => USAGE_CERT_ISSUE_TOKEN,
            Self::CertClaim => USAGE_CERT_CLAIM,
        }
    }
}

/// `indicatrix-worker --help` / `indicatrix-worker -h` / no arguments at all.
///
/// Small
/// enough to fit on a terminal screen; command-specific detail lives on
/// [`USAGE_RENDER`]/[`USAGE_SERVE`]/[`USAGE_CERT`], not here.
pub const USAGE_ROOT: &str =
    "indicatrix-worker -- headless indicatrix coordinator and render worker

USAGE:
    indicatrix-worker <render|serve|join|cert> [args...]

COMMANDS:
    render    Trace a scene straight to a PNG. No networking (worker feature).
    serve    The coordinator: serve indicatrix-net's design-library protocol,
              accept joining workers, and (with --render) render requests itself.
    join      Dial a coordinator's worker port and render its requests
              (worker feature).
    cert      Manage the private CA used for serve's and join's mutual TLS.

RELEASE NOTE: `serve` no longer renders by default -- add --render (or
--only-gpu/--only-cpu) to keep a single-worker setup rendering.

Run `indicatrix-worker <command> --help` for that command's own flags (and
`indicatrix-worker cert <sub-command> --help` for one of cert's five).
";

/// `indicatrix-worker render --help`.
pub const USAGE_RENDER: &str = "indicatrix-worker render -- trace a scene straight to a PNG. No networking.

USAGE:
    indicatrix-worker render --scene <scene.json> --out <render.png> --width <px> --height <px> --samples <n> [--threads <n>] [--only-gpu | --only-cpu]

    Needs a build with the `worker` feature; a build without it refuses `render`.

    --scene <path>    Path to a JSON-encoded scene (a indicatrix-net SceneState). Its own
                       width/height fields, if present, are ignored -- --width/--height
                       below are authoritative for the output image, so the same
                       scene.json can be re-rendered at different resolutions.
    --out <path>      Output PNG path. Parent directories are created if missing.
    --width <px>      Output image width, in pixels.
    --height <px>     Output image height, in pixels.
    --samples <n>     Total samples per pixel to trace.
    --threads <n>     CPU threads to use (default: all available cores). Only governs
                       the CPU tracer -- ignored by the GPU path itself (a single
                       compute-pipeline dispatch, not thread-parallel), so it still
                       applies whenever the GPU declines a request (no adapter,
                       --only-cpu, built without the gpu feature, or a scene past the
                       adapter's buffer limits) and this falls back to the CPU tracer.
    --only-gpu        GPU only -- never splits work onto the CPU tracer. Still falls
                       back to the CPU tracer for a request the GPU itself declines (no
                       adapter, or a scene past its buffer limits). Rejected at parse
                       time on a binary built without the gpu feature (there is no GPU
                       path to route through at all). Mutually exclusive with --only-cpu.
    --only-cpu        Force the CPU tracer even if this binary was built with the gpu
                       feature and a usable adapter is present. For A/B comparison
                       against the GPU path, or a machine whose adapter misbehaves.
                       Meaningless (and harmless) on a binary built without the gpu
                       feature, which never uses the GPU regardless. Mutually exclusive
                       with --only-gpu.
                       Default (neither flag given): hybrid -- CPU and GPU trace
                       concurrently whenever the measured split is worth it (see
                       render_core::hybrid), automatically falling back to GPU-only
                       when it isn't.
";

/// `indicatrix-worker serve --help`.
pub const USAGE_SERVE: &str = "indicatrix-worker serve -- the coordinator: serve the design library, accept
joining workers, and (with --render) render requests itself

USAGE:
    indicatrix-worker serve  [--bind <host:port>] [--allow-remote] [--db <path>] [--max-connections <n>] [--max-preauth-per-ip <n>]
                         [--render] [--threads <n>] [--only-gpu | --only-cpu]
                         --ca <ca.pem> --cert <server.pem> --key <server.key> [--allowlist <path>] [--trust-any-client-cert]
                         [--enroll-bind <host:port>] [--no-enroll]
                         [--worker-bind <host:port>] [--worker-enroll-bind <host:port>] [--worker-allowlist <path>] [--no-workers]
                         [--interactive-workers <n|all>] [--pin-interactive-worker <label>] [--max-job-memory-mib <n>]
                         [--whole-image-secs <secs>] [--whole-image-pixel-samples <n>] [--jobs-per-viewer <n>]
                         [--payload-encoding <auto|raw|lz4|zstd[:LEVEL]>]
    indicatrix-worker serve  [--bind <host:port>] [--render] [--threads <n>] [--only-gpu | --only-cpu] [--db <path>] [--max-connections <n>] --insecure-no-tls

    RELEASE NOTE: `serve` no longer renders by default. A single-worker setup that
    should keep rendering must add --render (or --only-gpu/--only-cpu, which imply it).

    Always serves indicatrix-net's read-only design-library protocol to viewers on
    --bind (default 7878) and accepts `indicatrix-worker join` render workers on the
    separate worker port (default 7880). It renders with its own CPU/GPU only with
    --render; the render protocol needs a build with the `worker` feature. WELCOME
    advertises what is actually available: no render capability at all for a bare
    coordinator with no joined workers, the plain CPU/GPU backend for --render alone,
    and Backend::Coordinator{workers, threads, gpus} once workers have joined.
    Requires mutual TLS by default -- see `indicatrix-worker cert --help`.

    PORTS: viewers 7878 + viewer enrollment 7879; workers 7880 + worker
    enrollment 7881. Two TLS listeners, two allowlists (allowlist-viewers.txt,
    allowlist-workers.txt); a viewer certificate is never accepted on the worker port
    and a worker certificate never on the viewer port (ROLE_REFUSED).

    --bind <addr>            Viewer address to listen on (default: 127.0.0.1:7878, loopback only).
    --db <path>              The design-library database to serve (see indicatrix-vault).
                              Opened read-only. Defaults to facet_diagrams.sqlite
                              resolved relative to the process's working directory.
    --render                  Render requests with this machine's own CPU/GPU lane (the
                               behaviour `serve` had by default before coordinator mode).
                               Without it no GPU is acquired and no sample is traced here.
    --threads <n>             CPU threads used per render request by the own lane (default:
                               all cores). Only governs the CPU tracer.
    --only-gpu                Own lane GPU only -- never splits work onto the CPU tracer.
                               Still falls back to the CPU tracer per request/sub-batch the
                               GPU itself declines. Implies --render. Rejected at parse
                               time on a binary built without the gpu feature. Mutually
                               exclusive with --only-cpu.
    --only-cpu                Own lane CPU only, even with a usable GPU adapter -- WELCOME
                               then reports Backend::Cpu. Implies --render. Mutually
                               exclusive with --only-gpu.
                               Default with --render alone: hybrid -- CPU and GPU trace
                               concurrently whenever the measured split is worth it (see
                               render_core::hybrid), else GPU-only.
    --allow-remote            Required to bind any non-loopback address (viewer, worker or
                               enrollment port), TLS or not.
    --ca <path>               CA certificate that issued --cert and every trusted client
                               certificate. Required unless --insecure-no-tls.
    --cert <path>             This coordinator's own certificate (see `indicatrix-worker
                               cert issue-server --help`). Used on both ports.
    --key <path>              This coordinator's own private key.
    --allowlist <path>        SHA-256 fingerprints of trusted VIEWER certificates, one per
                               line. Defaults to allowlist-viewers.txt next to --ca -- or,
                               while only that exists, the pre-role allowlist.txt (kept as
                               the viewers list; rename it whenever convenient).
    --trust-any-client-cert   Skip BOTH fingerprint allowlists -- trust any client whose
                               certificate chains to --ca and carries the port's role.
                               Off by default; skipping the allowlist must be explicit.
    --insecure-no-tls         Serve plaintext, no TLS, no authentication at all. Refused
                               on a non-loopback --bind. Disables the worker port and both
                               enrollment listeners. For local debugging only.
    --enroll-bind <host:port> VIEWER enrollment listener (see `cert issue-token --help`).
                               Defaults to the --bind host, one port up (7879).
    --no-enroll               Start neither enrollment listener (viewer or worker).
    --worker-bind <host:port> The worker port `indicatrix-worker join` dials. Defaults to
                               the --bind host, two ports up (7880). Worker client
                               certificates (`cert issue-client --role worker`) only.
    --worker-enroll-bind <host:port>
                              WORKER enrollment listener (`cert issue-token --role
                               worker`, claimed by `join --token`). Defaults to the worker
                               port's host, one port up (7881).
    --worker-allowlist <path> SHA-256 fingerprints of trusted WORKER certificates.
                               Defaults to allowlist-workers.txt next to --ca. May not
                               exist yet: every worker connection is then refused until
                               the first worker is enrolled.
    --no-workers              Don't open the worker port or its enrollment listener.
    --max-connections <n>     Most connections handled at once (default 64), counted
                               separately for viewers and for joined workers. A connection
                               past the cap gets a definitive error reply. At least 1.
    --max-preauth-per-ip <n>  Most viewer-port connections one source IP address may have
                               open that have not finished authenticating (default 8).
                               A connection counts only until its TLS handshake and
                               allowlist check succeed; after that it counts against
                               --max-connections alone. Over-cap connections are closed
                               at once and logged. At least 1.

    RENDERING OVER JOINED WORKERS (worker builds): a large export-type request (Batch)
    is split into chunks over every idle joined worker whose pixel cap accepts it plus
    the own lane; a small one (see --whole-image-secs) is one picture on the fastest idle
    joined worker. A live-view request (Interactive) runs on the own lane and every idle
    joined worker; without --render it takes the fastest idle worker alone.

    --interactive-workers <n|all>
                              ADVANCED. Give each live-view request up to n of the
                               fastest idle joined workers besides the own lane (default
                               all: every idle one). 0 keeps the live view on the own
                               lane alone, the lowest-latency path for a slow link.
    --pin-interactive-worker <label>
                              ADVANCED. When a live-view request takes joined workers
                               (no --render, or --interactive-workers above 0), use the
                               worker whose certificate label is <label> (the --name its
                               worker certificate was issued with; `worker:<label>` also
                               works) first, while it is connected, idle and accepts the
                               image. Otherwise the fastest-idle-worker rule applies
                               (logged).
    --whole-image-secs <secs> A Batch request estimated to render in less than this on
                               the fastest eligible joined worker (default 2) is rendered
                               whole on one lane: the fastest idle joined worker, the
                               own lane only while none is idle.
    --whole-image-pixel-samples <n>
                              The same rule by size while no joined worker has a
                               measured rate yet: width x height x samples at most n
                               (default 67108864).
    --jobs-per-viewer <n>     Most whole-image jobs one viewer certificate has running
                               at once (default 8). At least 1.
    --payload-encoding <auto|raw|lz4|zstd[:LEVEL]>
                              How the radiance and picture payloads sent to each viewer
                               are compressed. auto (default): follow each connection's
                               measured write speed (zstd or LZ4 on slow links, raw on
                               fast ones; a loopback peer always gets raw). raw, lz4,
                               zstd (level 1) or zstd:LEVEL (1-22) pin one encoding on
                               every link, loopback included, when the viewer accepts it.
    --max-job-memory-mib <n>  Cap on the buffers of all in-flight multi-lane jobs
                               (default 2048). Each job is charged width x height x 48
                               bytes, plus width x height x 36 bytes for every lane
                               (the joined workers and the own lane), plus its HDR map
                               and any viewer contribution; a job whose lanes do not all
                               fit runs on fewer, and one past the cap even with a single
                               lane is refused, not queued. A maximum-size 8K job needs
                               about 2700 MiB with one lane.

    HDR ENVIRONMENT MAPS (worker builds): a scene lit by an HDR map names it by hash.
    With --render or a worker port the coordinator keeps a bounded on-disk cache
    (asset-cache next to --db, or $INDICATRIX_ASSET_CACHE_DIR; size cap
    $INDICATRIX_ASSET_CACHE_MIB MiB, default 2048), asks the viewer once for a map it
    lacks, and forwards it to each joined worker that asks. HDR jobs only go to joined
    workers whose own cache works; WELCOME advertises HDR when some lane can render it.
";

/// `indicatrix-worker join --help`.
pub const USAGE_JOIN: &str = "indicatrix-worker join -- dial a coordinator and render its requests

USAGE:
    indicatrix-worker join <coordinator-host:port> [--cert-dir <dir>] [--slots <k>] [--threads <n>] [--only-gpu | --only-cpu]
                           [--payload-encoding <auto|raw|lz4|zstd[:LEVEL]>]
    indicatrix-worker join --coordinator <host:port> --token <GW1-...> [--enroll-addr <host:port>] [--cert-dir <dir>] ...

    Connects OUT to a coordinator's worker port (`serve`, default port 7880) over mutual
    TLS with a WORKER client certificate, reports this machine's render capability, and
    then serves the coordinator's render requests on that connection -- no inbound port
    is needed here, so this works from behind NAT or a cloud VM. Reconnects forever
    with jittered backoff (1 s doubling to 60 s) whenever the connection drops or is
    refused. Needs a build with the `worker` feature.

    <coordinator-host:port>   The coordinator's WORKER port (also as --coordinator).
    --cert-dir <dir>          The worker certificate bundle (ca.pem, client.pem,
                               client.key; default: worker-cert in the working
                               directory). With --token, the claimed bundle is written
                               here first.
    --token <GW1-...>         Claim a WORKER enrollment token (`cert issue-token --role
                               worker` on the coordinator host) before joining.
    --enroll-addr <host:port> The coordinator's WORKER enrollment listener (default: the
                               coordinator host, one port above the worker port -- 7881).
    --slots <k>               Parallel connections (default 2), each serving one request
                               stream at a time. A second slot keeps a chunk in flight
                               while another one's transfer/decode gap would otherwise
                               idle a fast worker; higher k mainly helps CPU-only
                               machines juggling many small chunks. At most 64.
    --threads <n>             CPU tracer threads (default: all cores).
    --only-gpu                GPU only (rejected without the gpu feature). Mutually
                               exclusive with --only-cpu.
    --only-cpu                CPU only, even with a usable GPU adapter.
    --payload-encoding <auto|raw|lz4|zstd[:LEVEL]>
                              How the frames sent to the coordinator are compressed.
                               auto (default): follow the measured speed of each
                               connection (zstd or LZ4 on slow links, raw on fast ones;
                               a loopback coordinator always gets raw). raw, lz4, zstd
                               (level 1) or zstd:LEVEL (1-22) pin one encoding.

    HDR ENVIRONMENT MAPS: a scene lit by an HDR map names it by hash; this worker asks
    the coordinator for a map it lacks (once) and keeps it in a bounded on-disk cache:
    asset-cache next to --cert-dir (worker-cert -> ./asset-cache), or
    $INDICATRIX_ASSET_CACHE_DIR; size cap $INDICATRIX_ASSET_CACHE_MIB MiB (default
    2048). If the cache cannot be opened, the worker tells the coordinator it renders
    no HDR scenes and only gets studio-lit work.
";

/// `indicatrix-worker cert --help`.
pub const USAGE_CERT: &str =
    "indicatrix-worker cert -- manage the private CA used for serve's and join's mutual TLS

USAGE:
    indicatrix-worker cert <init|issue-server|issue-client|issue-token|claim> [args...]

    Manages an in-process private CA for the coordinator's mutual TLS. Client
    certificates carry a role: viewer (indicatrix-cut) or worker (`indicatrix-worker
    join`), each with its own allowlist (allowlist-viewers.txt, allowlist-workers.txt).
    There is no CRL or OCSP -- revoking a client is deleting its line from the allowlist
    `issue-client` (or a successful claim) added it to (see `indicatrix-worker serve
    --help`'s --allowlist and --worker-allowlist entries).

COMMANDS:
    init            Generate a new CA keypair and self-signed certificate.
    issue-server    Issue the coordinator's (serve's) own certificate, signed by the CA.
    issue-client    Issue one viewer's or worker's certificate onto a bundle, BY HAND.
    issue-token     Mint a one-time enrollment token from a running `serve`.
    claim           Redeem an enrollment token on the machine being enrolled.

Run `indicatrix-worker cert <command> --help` for that command's own flags.
";

/// `indicatrix-worker cert init --help`.
pub const USAGE_CERT_INIT: &str = "indicatrix-worker cert init -- generate a new CA keypair

USAGE:
    indicatrix-worker cert init --dir <pki-dir>

    Generates a new CA keypair and self-signed certificate in --dir. Refuses to run if
    --dir already has one (regenerating it would invalidate every certificate already
    issued from it).

    --dir <path>      The private CA's directory (ca.pem, ca.key, allowlists).
";

/// `indicatrix-worker cert issue-server --help`.
pub const USAGE_CERT_ISSUE_SERVER: &str =
    "indicatrix-worker cert issue-server -- issue the coordinator's own TLS certificate

USAGE:
    indicatrix-worker cert issue-server --dir <pki-dir> --host <name> [--host <name> ...] --ip <addr> [--ip <addr> ...]

    Issues the certificate `serve` presents (its --cert/--key) on every listener it
    opens, signed by the CA in --dir. --host/--ip become the certificate's Subject
    Alternative Names and may each be repeated; at least one of either is required --
    TLS ignores Common Name entirely, so a viewer or joining worker connecting by IP
    needs an IP SAN specifically, not just a DNS name.

    --dir <path>      The private CA's directory (ca.pem, ca.key, allowlists).
    --host <name>     A DNS Subject Alternative Name. Repeatable.
    --ip <addr>       An IP Subject Alternative Name. Repeatable.
";

/// `indicatrix-worker cert issue-client --help`.
pub const USAGE_CERT_ISSUE_CLIENT: &str =
    "indicatrix-worker cert issue-client -- issue one viewer's or worker's TLS certificate

USAGE:
    indicatrix-worker cert issue-client --dir <pki-dir> --name <label> --out <bundle-dir> [--role viewer|worker]

    Issues one client certificate, signed by the CA in --dir, and writes a
    self-contained bundle (ca.pem, client.pem, client.key) to --out for copying to that
    machine BY HAND. --name labels the certificate's subject and the allowlist entry
    this also adds immediately. Still here, unchanged, for air-gapped or offline setups
    where nothing can dial out to a running `serve`.

    --dir <path>      The private CA's directory (ca.pem, ca.key, allowlists).
    --name <label>    A human label for the certificate and allowlist entry. A viewer
                       name may not start with \"worker:\".
    --out <path>      Where to write the certificate bundle.
    --role <role>     viewer (default): a viewer certificate, Common Name <label>, added
                       to allowlist-viewers.txt (or a pre-role allowlist.txt while only
                       that exists). worker: a certificate for `indicatrix-worker join`,
                       Common Name worker:<label>, added to allowlist-workers.txt. A
                       listener checks both the role and its own allowlist.
";

/// `indicatrix-worker cert issue-token --help`.
pub const USAGE_CERT_ISSUE_TOKEN: &str =
    "indicatrix-worker cert issue-token -- mint a one-time enrollment token

USAGE:
    indicatrix-worker cert issue-token --ca <ca.pem> --admin-addr <host:port> --name <label> [--role viewer|worker]

    Asks a RUNNING `serve` process's enrollment listener to mint a one-time, 180-second
    enrollment token for a viewer (or, with --role worker, a joining worker) named
    --name, and prints it for the operator to read or send to whoever is enrolling.
    Connects using --ca (the operator's own already-possessed CA file -- ordinary TLS
    verification, no bootstrap problem on this side) to --admin-addr, which the running
    `serve` process logged at startup (see `indicatrix-worker serve --help`'s
    --enroll-bind and --worker-enroll-bind entries) and only honors from a loopback
    connection, regardless of --admin-addr. Mints the certificate bundle immediately but
    holds it in that `serve` process's memory only -- nothing is written to disk, and
    nothing is added to either allowlist, until the token is claimed (`indicatrix-worker
    cert claim --help`, or `indicatrix-worker join --help`'s --token for a worker).

    A loopback connection alone does not authorise issuing: the request also presents the
    operator secret in `issue.secret`, which `serve` creates in the PKI directory (the
    directory holding --ca) on its first start, readable only by its owner. This command
    reads that file, so run it as the user who can read the PKI directory, on the
    coordinator host. A `serve` that predates the operator secret must be updated first.

    --ca <path>           The CA certificate to verify the enrollment listener against --
                           ordinary verification, not pinned, since the operator already
                           has this file. `issue.secret` is read from the same directory.
    --admin-addr <addr>   The running coordinator's enrollment listener address: the
                           viewer one (default port 7879) for --role viewer, the worker
                           one (default port 7881) for --role worker.
    --name <label>        A human label for the certificate and allowlist entry.
    --role <role>         viewer (default) or worker. A worker token is claimed with
                           `indicatrix-worker join --token ...` and yields a worker
                           certificate (Common Name worker:<label>, allowlist-workers.txt).
                           A listener refuses a token request for the other role.
";

/// `indicatrix-worker cert claim --help`.
pub const USAGE_CERT_CLAIM: &str = "indicatrix-worker cert claim -- redeem an enrollment token

USAGE:
    indicatrix-worker cert claim --token <token> --addr <host:port> --out <bundle-dir>

    Redeems a token from `cert issue-token`, on the machine being enrolled: connects to
    --addr (the coordinator's enrollment listener), verifies it presents a certificate
    chain rooted at the CA fingerprint the token itself carries -- BEFORE sending the
    token's secret, so an attacker who isn't the real coordinator can't collect a valid
    client certificate by impersonating it -- and on success writes the same three-file
    bundle (ca.pem, client.pem, client.key) to --out that `cert issue-client` would have:
    a viewer bundle for indicatrix-cut's certificate folder, or a worker bundle for
    `join --cert-dir` (`join --token` claims a worker token itself). Only works once per
    token: a second claim with the same token fails, whether or not the first one
    succeeded.

    --token <token>   The token text printed by `cert issue-token`.
    --addr <addr>     The coordinator's enrollment listener for the token's role: the
                       viewer one (default port 7879) or the worker one (default port
                       7881).
    --out <path>      Where to write the claimed certificate bundle (viewer or worker).
";
