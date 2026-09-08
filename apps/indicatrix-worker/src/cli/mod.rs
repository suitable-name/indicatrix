//! Hand-rolled argument parsing -- no external CLI-parsing crate.
//!
//! The surface is small (two subcommands, five flags each at most) and every value
//! needs its own validation message anyway, so a dependency buys little.

mod args;
mod parse;
#[cfg(test)]
mod tests;

pub use args::{
    CertClaimArgs, CertInitArgs, CertIssueClientArgs, CertIssueServerArgs, CertIssueTokenArgs,
    Command, ComputeMode, HelpTopic, RenderArgs, ServeArgs,
};
pub use parse::parse;

pub const DEFAULT_BIND: &str = "127.0.0.1:7878";

/// `serve --max-connections`'s default -- see [`ServeArgs::max_connections`]'s doc
/// comment and `serve::ConnectionLimiter`.
pub const DEFAULT_MAX_CONNECTIONS: usize = 64;

impl HelpTopic {
    /// The `--help` page text for this topic. `main.rs` prints exactly this and
    /// nothing else -- each page is meant to stand alone on a terminal screen.
    #[must_use]
    pub const fn usage(&self) -> &'static str {
        match self {
            Self::Root => USAGE_ROOT,
            Self::Render => USAGE_RENDER,
            Self::Serve => USAGE_SERVE,
            Self::Cert => USAGE_CERT,
            Self::CertInit => USAGE_CERT_INIT,
            Self::CertIssueServer => USAGE_CERT_ISSUE_SERVER,
            Self::CertIssueClient => USAGE_CERT_ISSUE_CLIENT,
            Self::CertIssueToken => USAGE_CERT_ISSUE_TOKEN,
            Self::CertClaim => USAGE_CERT_CLAIM,
        }
    }
}

/// `indicatrix-worker --help` / `indicatrix-worker -h` / no arguments at all. Small
/// enough to fit on a terminal screen; command-specific detail lives on
/// [`USAGE_RENDER`]/[`USAGE_SERVE`]/[`USAGE_CERT`], not here.
pub const USAGE_ROOT: &str = "indicatrix-worker -- headless indicatrix render worker

USAGE:
    indicatrix-worker <render|serve|cert> [args...]

COMMANDS:
    render    Trace a scene straight to a PNG. No networking.
    serve     Serve indicatrix-net's design-library protocol (and, with the
              worker feature, render requests) over TCP.
    cert      Manage the private CA used for SERVE's mutual TLS.

Run `indicatrix-worker <command> --help` for that command's own flags (and
`indicatrix-worker cert <sub-command> --help` for one of cert's five).
";

/// `indicatrix-worker render --help`.
pub const USAGE_RENDER: &str = "indicatrix-worker render -- trace a scene straight to a PNG. No networking.

USAGE:
    indicatrix-worker render --scene <scene.json> --out <render.png> --width <px> --height <px> --samples <n> [--threads <n>] [--only-gpu | --only-cpu]

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
                       --only-cpu, built without the gpu feature, or an unsupported
                       material) and this falls back to the CPU tracer.
    --only-gpu        GPU only -- never splits work onto the CPU tracer. Still falls
                       back to the CPU tracer for a request the GPU itself declines (no
                       adapter, or an unsupported material). Rejected at parse time on
                       a binary built without the gpu feature (there is no GPU path to
                       route through at all). Mutually exclusive with --only-cpu.
    --only-cpu        Force the CPU tracer even if this binary was built with the gpu
                       feature and a usable adapter is present. For A/B comparison
                       against the GPU path, or a machine whose adapter misbehaves.
                       Meaningless (and harmless) on a binary built without the gpu
                       feature, which never uses the GPU regardless. Mutually exclusive
                       with --only-gpu. What --no-gpu (removed) used to do.
                       Default (neither flag given): hybrid -- CPU and GPU trace
                       concurrently whenever the measured split is worth it (see
                       render_core::hybrid), automatically falling back to GPU-only
                       when it isn't.
";

/// `indicatrix-worker serve --help`.
pub const USAGE_SERVE: &str = "indicatrix-worker serve -- serve indicatrix-net's design-library protocol (and,
with the worker feature, render requests) over TCP

USAGE:
    indicatrix-worker serve  [--bind <host:port>] [--threads <n>] [--allow-remote] [--only-gpu | --only-cpu] [--db <path>] [--max-connections <n>]
                         --ca <ca.pem> --cert <server.pem> --key <server.key> [--allowlist <path>] [--trust-any-client-cert]
                         [--enroll-bind <host:port>] [--no-enroll]
    indicatrix-worker serve  [--bind <host:port>] [--threads <n>] [--allow-remote] [--only-gpu | --only-cpu] [--db <path>] [--max-connections <n>] --insecure-no-tls

    Serves indicatrix-net's read-only design-library protocol over TCP (listing/searching
    designs, fetching one in full, fetching an attachment). A build with the `worker`
    feature also accepts RenderRequests and replies with traced radiance, so a viewer
    can offload samples to this process. Requires mutual TLS by default -- both sides
    must present a certificate signed by the same private CA (see `indicatrix-worker cert
    --help` for how to create one). WELCOME advertises exactly which of the two this
    instance actually supports.

    --bind <addr>            Address to listen on (default: 127.0.0.1:7878, loopback only).
    --db <path>              The design-library database to serve (see indicatrix-vault).
                              Opened read-only when possible. Defaults to
                              facet_diagrams.sqlite resolved relative to the process's
                              working directory -- unchanged from before this flag
                              existed; --db is an addition for long-running servers, not
                              a replacement for that default.
    --threads <n>             CPU threads used per render request (default: all cores).
                               Only governs the CPU tracer -- ignored by the GPU path
                               itself (a single compute-pipeline dispatch, not thread-
                               parallel), so it still applies whenever a request falls
                               back to the CPU tracer (no adapter, --only-cpu, built
                               without the gpu feature, or an unsupported material).
    --only-gpu                GPU only -- never splits work onto the CPU tracer for any
                               request, even when the hybrid split (CPU and GPU tracing
                               concurrently whenever the measured split is worth it --
                               see render_core::hybrid) would otherwise have offered it
                               one. Still falls back to the CPU tracer per request/
                               sub-batch the GPU itself declines. Rejected at parse time
                               on a binary built without the gpu feature. Mutually
                               exclusive with --only-cpu.
    --only-cpu                Force the CPU tracer for every request, even if this
                               binary was built with the gpu feature and a usable
                               adapter is present -- WELCOME then reports Backend::Cpu.
                               For A/B comparison, or a machine whose adapter misbehaves.
                               Mutually exclusive with --only-gpu. What --no-gpu
                               (removed) used to do.
                               Default (neither flag given): hybrid -- CPU and GPU trace
                               concurrently whenever the measured split is worth it (see
                               render_core::hybrid), automatically falling back to
                               GPU-only when it isn't.
    --allow-remote            Required to bind any non-loopback address, TLS or not.
    --ca <path>               CA certificate that issued both --cert and every trusted
                               client certificate. Required unless --insecure-no-tls.
    --cert <path>             This worker's own certificate (see `indicatrix-worker cert
                               issue-server --help`).
    --key <path>              This worker's own private key.
    --allowlist <path>        SHA-256 fingerprints of trusted client certificates, one
                               per line (see `indicatrix-worker cert issue-client --help`).
                               Defaults to allowlist.txt next to --ca.
    --trust-any-client-cert   Skip the fingerprint allowlist -- trust any client whose
                               certificate chains to --ca. Off by default: the allowlist
                               is what actually decides which signed clients may connect,
                               not just which CA signed them, so skipping it is an
                               explicit, visible choice, never a silent default.
    --insecure-no-tls         Serve plaintext, no TLS, no authentication at all. Refused
                               on a non-loopback --bind. A warning is logged for every
                               connection accepted this way. For local debugging only.
    --enroll-bind <host:port> Address for the token-based enrollment listener (see
                               `indicatrix-worker cert issue-token --help` and `indicatrix-worker
                               cert claim --help`). Defaults to the same host as --bind,
                               one port up. Ignored with --insecure-no-tls (there is no
                               CA to enroll against). Follows the same loopback /
                               --allow-remote gate as --bind.
    --no-enroll               Don't start the enrollment listener at all -- the manual
                               `cert issue-client` + copy-the-bundle path (see
                               `indicatrix-worker cert issue-client --help`) still works,
                               this just opens no extra port for it.
    --max-connections <n>     Most connections this worker handles at once (default 64).
                               One cap for the one listener, regardless of transport (TLS
                               or --insecure-no-tls) or build mode (library-only or
                               worker). A connection past the cap is still accepted and
                               given a definitive error reply instead of being left to
                               hang or silently reset. Must be at least 1.
";

/// `indicatrix-worker cert --help`.
pub const USAGE_CERT: &str =
    "indicatrix-worker cert -- manage the private CA used for SERVE's mutual TLS

USAGE:
    indicatrix-worker cert <init|issue-server|issue-client|issue-token|claim> [args...]

    Manages an in-process private CA for SERVE's mutual TLS. There is no CRL or OCSP --
    revoking a client is deleting its line from the allowlist file `issue-client` (or a
    successful `cert claim`) wrote to (see `indicatrix-worker serve --help`'s --allowlist
    entry).

COMMANDS:
    init            Generate a new CA keypair and self-signed certificate.
    issue-server    Issue this worker's own certificate, signed by the CA.
    issue-client    Issue one viewer's certificate onto a bundle, BY HAND.
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

    --dir <path>      The private CA's directory (ca.pem, ca.key, allowlist.txt).
";

/// `indicatrix-worker cert issue-server --help`.
pub const USAGE_CERT_ISSUE_SERVER: &str =
    "indicatrix-worker cert issue-server -- issue this worker's own TLS certificate

USAGE:
    indicatrix-worker cert issue-server --dir <pki-dir> --host <name> [--host <name> ...] --ip <addr> [--ip <addr> ...]

    Issues this worker's certificate, signed by the CA in --dir. --host/--ip become the
    certificate's Subject Alternative Names and may each be repeated; at least one of
    either is required -- TLS ignores Common Name entirely, so a viewer connecting by IP
    needs an IP SAN specifically, not just a DNS name.

    --dir <path>      The private CA's directory (ca.pem, ca.key, allowlist.txt).
    --host <name>     A DNS Subject Alternative Name. Repeatable.
    --ip <addr>       An IP Subject Alternative Name. Repeatable.
";

/// `indicatrix-worker cert issue-client --help`.
pub const USAGE_CERT_ISSUE_CLIENT: &str =
    "indicatrix-worker cert issue-client -- issue one viewer's TLS certificate

USAGE:
    indicatrix-worker cert issue-client --dir <pki-dir> --name <label> --out <bundle-dir>

    Issues one viewer's certificate, signed by the CA in --dir, and writes a
    self-contained bundle (ca.pem, client.pem, client.key) to --out for copying to that
    viewer's machine BY HAND. --name labels the certificate's subject and the allowlist
    entry this also adds immediately. Still here, unchanged, for air-gapped or offline
    setups where nothing can dial out to a running `serve`.

    --dir <path>      The private CA's directory (ca.pem, ca.key, allowlist.txt).
    --name <label>    A human label for the certificate and allowlist entry.
    --out <path>      Where to write the viewer's certificate bundle.
";

/// `indicatrix-worker cert issue-token --help`.
pub const USAGE_CERT_ISSUE_TOKEN: &str =
    "indicatrix-worker cert issue-token -- mint a one-time enrollment token

USAGE:
    indicatrix-worker cert issue-token --ca <ca.pem> --admin-addr <host:port> --name <label>

    Asks a RUNNING `serve` process's enrollment listener to mint a one-time, 180-second
    enrollment token for a viewer named --name, and prints it for the operator to read
    or send to whoever is enrolling. Connects using --ca (the operator's own already-
    possessed CA file -- ordinary TLS verification, no bootstrap problem on this side)
    to --admin-addr, which the running `serve` process logged at startup (see
    `indicatrix-worker serve --help`'s --enroll-bind entry) and only honors from a loopback
    connection, regardless of --admin-addr. Mints the certificate bundle immediately but
    holds it in that `serve` process's memory only -- nothing is written to disk, and
    nothing is added to the allowlist, until the token is claimed (see `indicatrix-worker
    cert claim --help`).

    --ca <path>           The CA certificate to verify the enrollment listener against --
                           ordinary verification, not pinned, since the operator already
                           has this file.
    --admin-addr <addr>   The running worker's enrollment listener address.
    --name <label>        A human label for the certificate and allowlist entry.
";

/// `indicatrix-worker cert claim --help`.
pub const USAGE_CERT_CLAIM: &str = "indicatrix-worker cert claim -- redeem an enrollment token

USAGE:
    indicatrix-worker cert claim --token <token> --addr <host:port> --out <bundle-dir>

    Redeems a token from `cert issue-token`, on the machine being enrolled: connects to
    --addr (the enrollment listener), verifies it presents a certificate chain rooted at
    the CA fingerprint the token itself carries -- BEFORE sending the token's secret, so
    an attacker who isn't the real worker can't collect a valid client certificate by
    impersonating it -- and on success writes the same three-file bundle (ca.pem,
    client.pem, client.key) to --out that `cert issue-client` would have, so
    indicatrix-cut's WorkerSettings.cert_dir keeps working unchanged either way. Only works
    once per token: a second `cert claim` with the same token fails, whether or not the
    first one succeeded.

    --token <token>   The token text printed by `cert issue-token`.
    --addr <addr>     The worker's enrollment listener address.
    --out <path>      Where to write the viewer's certificate bundle.
";
