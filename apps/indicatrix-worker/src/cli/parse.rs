//! The actual argument-parsing logic: [`parse`] and one `parse_*` function per
//! subcommand (and cert sub-subcommand).
//!
//! # Why `Result<_, String>`, not a typed error
//!
//! Unlike `crate::pki`/`crate::enroll`, every error here is terminal, user-facing CLI
//! diagnostic text -- `main.rs` prints it straight to stderr next to the relevant
//! `--help` page and exits; nothing ever matches on a parse failure's kind. A typed enum
//! would add a variant per malformed flag for no caller that needs one, so this module
//! keeps returning ready-to-print `String`s instead.

use super::args::{
    CertClaimArgs, CertInitArgs, CertIssueClientArgs, CertIssueServerArgs, CertIssueTokenArgs,
    Command, ComputeMode, HelpTopic, JoinArgs, RenderArgs, ServeArgs,
};
use indicatrix_net::messages::PeerRole;
use std::{net::IpAddr, path::PathBuf};

impl HelpTopic {
    /// Resolves the `--help` page `argv` names, from the leading run of non-flag
    /// tokens (`argv[0]`, and `argv[1]` when `argv[0]` is `"cert"`) -- the same prefix
    /// [`parse`] dispatches on. An unrecognized or empty leading run resolves to
    /// [`HelpTopic::Root`] (or [`HelpTopic::Cert`] one level down): the page that lists
    /// the valid names.
    pub fn from_argv(argv: &[String]) -> Self {
        let mut leading = argv.iter().take_while(|a| !a.starts_with('-'));
        match leading.next().map(String::as_str) {
            Some("render") => Self::Render,
            Some("serve") => Self::Serve,
            Some("join") => Self::Join,
            Some("cert") => match leading.next().map(String::as_str) {
                Some("init") => Self::CertInit,
                Some("issue-server") => Self::CertIssueServer,
                Some("issue-client") => Self::CertIssueClient,
                Some("issue-token") => Self::CertIssueToken,
                Some("claim") => Self::CertClaim,
                _ => Self::Cert,
            },
            _ => Self::Root,
        }
    }
}

/// Parses `--only-gpu`/`--only-cpu` for one subcommand's flag loop: `flag` is whichever
/// of the two was just seen, `compute_mode` holds whatever the same loop already parsed.
/// Errors if the two conflict; repeating the same flag is fine (idempotent).
fn parse_compute_mode_flag(
    flag: ComputeMode,
    compute_mode: &mut Option<ComputeMode>,
) -> Result<(), String> {
    match compute_mode {
        Some(existing) if *existing != flag => Err(
            "--only-gpu and --only-cpu are mutually exclusive -- pass at most one (see --help)"
                .to_string(),
        ),
        _ => {
            *compute_mode = Some(flag);
            Ok(())
        }
    }
}

/// Resolves a subcommand's parsed (possibly absent) `--only-gpu`/`--only-cpu` flag to
/// a [`ComputeMode`]: `None` becomes [`ComputeMode::default`] (`Hybrid`). Rejects
/// `OnlyGpu` on a build without the `gpu` feature, where it would otherwise silently
/// behave like `OnlyCpu`.
fn resolve_compute_mode(compute_mode: Option<ComputeMode>) -> Result<ComputeMode, String> {
    let mode = compute_mode.unwrap_or_default();
    if matches!(mode, ComputeMode::OnlyGpu) && !cfg!(feature = "gpu") {
        return Err(
            "--only-gpu requires this binary to be built with the `gpu` feature -- \
             without it, there is no GPU path for this worker to ever route through \
             (see --help)"
                .to_string(),
        );
    }
    Ok(mode)
}

/// Parses `argv` (without the program name).
///
/// `-h`/`--help` anywhere in the arguments short-circuits to [`Command::Help`], even if
/// other required flags are missing. The [`HelpTopic`] is resolved by
/// [`HelpTopic::from_argv`], so `cert claim --help` reaches the claim page specifically.
///
/// # Errors
///
/// Returns a human-readable message (never a usage string -- callers print the
/// relevant `--help` page separately) describing the first thing wrong with `argv`.
pub fn parse(argv: &[String]) -> Result<Command, String> {
    if argv.is_empty() || argv.iter().any(|a| a == "-h" || a == "--help") {
        return Ok(Command::Help(HelpTopic::from_argv(argv)));
    }
    match argv[0].as_str() {
        "render" => parse_render(&argv[1..]).map(Command::Render),
        "serve" => parse_serve(&argv[1..]).map(|args| Command::Serve(Box::new(args))),
        "join" => parse_join(&argv[1..]).map(Command::Join),
        "cert" => parse_cert(&argv[1..]),
        other => Err(format!(
            "unknown subcommand {other:?} (expected \"render\", \"serve\", \"join\", or \"cert\"; see --help)"
        )),
    }
}

fn parse_cert(args: &[String]) -> Result<Command, String> {
    if args.is_empty() {
        return Err(
            "\"cert\" requires a sub-subcommand: \"init\", \"issue-server\", \"issue-client\", \
                    \"issue-token\", or \"claim\" (see --help)"
                .to_string(),
        );
    }
    match args[0].as_str() {
        "init" => parse_cert_init(&args[1..]).map(Command::CertInit),
        "issue-server" => parse_cert_issue_server(&args[1..]).map(Command::CertIssueServer),
        "issue-client" => parse_cert_issue_client(&args[1..]).map(Command::CertIssueClient),
        "issue-token" => parse_cert_issue_token(&args[1..]).map(Command::CertIssueToken),
        "claim" => parse_cert_claim(&args[1..]).map(Command::CertClaim),
        other => Err(format!(
            "unknown \"cert\" sub-subcommand {other:?} (expected \"init\", \"issue-server\", \"issue-client\", \
             \"issue-token\", or \"claim\"; see --help)"
        )),
    }
}

fn parse_cert_init(args: &[String]) -> Result<CertInitArgs, String> {
    let mut dir = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--dir" => {
                i += 1;
                dir = Some(PathBuf::from(arg_at(args, i, "--dir")?));
            }
            other => {
                return Err(format!(
                    "unknown flag {other:?} for \"cert init\" (see --help)"
                ));
            }
        }
        i += 1;
    }
    Ok(CertInitArgs {
        dir: dir.ok_or_else(|| "\"cert init\" requires --dir <path>".to_string())?,
    })
}

fn parse_cert_issue_server(args: &[String]) -> Result<CertIssueServerArgs, String> {
    let mut dir = None;
    let mut hosts = Vec::new();
    let mut ips = Vec::new();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--dir" => {
                i += 1;
                dir = Some(PathBuf::from(arg_at(args, i, "--dir")?));
            }
            "--host" => {
                i += 1;
                hosts.push(arg_at(args, i, "--host")?.to_string());
            }
            "--ip" => {
                i += 1;
                let raw = arg_at(args, i, "--ip")?;
                ips.push(
                    raw.parse::<IpAddr>()
                        .map_err(|_| format!("--ip expects an IP address, got {raw:?}"))?,
                );
            }
            other => {
                return Err(format!(
                    "unknown flag {other:?} for \"cert issue-server\" (see --help)"
                ));
            }
        }
        i += 1;
    }
    Ok(CertIssueServerArgs {
        dir: dir.ok_or_else(|| "\"cert issue-server\" requires --dir <path>".to_string())?,
        hosts,
        ips,
    })
}

fn parse_cert_issue_client(args: &[String]) -> Result<CertIssueClientArgs, String> {
    let mut dir = None;
    let mut name = None;
    let mut out = None;
    let mut role = PeerRole::Viewer;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--dir" => {
                i += 1;
                dir = Some(PathBuf::from(arg_at(args, i, "--dir")?));
            }
            "--name" => {
                i += 1;
                name = Some(arg_at(args, i, "--name")?.to_string());
            }
            "--out" => {
                i += 1;
                out = Some(PathBuf::from(arg_at(args, i, "--out")?));
            }
            "--role" => {
                i += 1;
                role = parse_role(arg_at(args, i, "--role")?)?;
            }
            other => {
                return Err(format!(
                    "unknown flag {other:?} for \"cert issue-client\" (see --help)"
                ));
            }
        }
        i += 1;
    }
    Ok(CertIssueClientArgs {
        dir: dir.ok_or_else(|| "\"cert issue-client\" requires --dir <path>".to_string())?,
        name: name.ok_or_else(|| "\"cert issue-client\" requires --name <label>".to_string())?,
        out: out.ok_or_else(|| "\"cert issue-client\" requires --out <path>".to_string())?,
        role,
    })
}

fn parse_cert_issue_token(args: &[String]) -> Result<CertIssueTokenArgs, String> {
    let mut ca = None;
    let mut admin_addr = None;
    let mut name = None;
    let mut role = PeerRole::Viewer;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--ca" => {
                i += 1;
                ca = Some(PathBuf::from(arg_at(args, i, "--ca")?));
            }
            "--admin-addr" => {
                i += 1;
                admin_addr = Some(arg_at(args, i, "--admin-addr")?.to_string());
            }
            "--name" => {
                i += 1;
                name = Some(arg_at(args, i, "--name")?.to_string());
            }
            "--role" => {
                i += 1;
                role = parse_role(arg_at(args, i, "--role")?)?;
            }
            other => {
                return Err(format!(
                    "unknown flag {other:?} for \"cert issue-token\" (see --help)"
                ));
            }
        }
        i += 1;
    }
    Ok(CertIssueTokenArgs {
        ca: ca.ok_or_else(|| "\"cert issue-token\" requires --ca <path>".to_string())?,
        admin_addr: admin_addr
            .ok_or_else(|| "\"cert issue-token\" requires --admin-addr <host:port>".to_string())?,
        name: name.ok_or_else(|| "\"cert issue-token\" requires --name <label>".to_string())?,
        role,
    })
}

fn parse_cert_claim(args: &[String]) -> Result<CertClaimArgs, String> {
    let mut token = None;
    let mut addr = None;
    let mut out = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--token" => {
                i += 1;
                token = Some(arg_at(args, i, "--token")?.to_string());
            }
            "--addr" => {
                i += 1;
                addr = Some(arg_at(args, i, "--addr")?.to_string());
            }
            "--out" => {
                i += 1;
                out = Some(PathBuf::from(arg_at(args, i, "--out")?));
            }
            other => {
                return Err(format!(
                    "unknown flag {other:?} for \"cert claim\" (see --help)"
                ));
            }
        }
        i += 1;
    }
    Ok(CertClaimArgs {
        token: token.ok_or_else(|| "\"cert claim\" requires --token <token>".to_string())?,
        addr: addr.ok_or_else(|| "\"cert claim\" requires --addr <host:port>".to_string())?,
        out: out.ok_or_else(|| "\"cert claim\" requires --out <path>".to_string())?,
    })
}

/// Parses a `--role` value: `viewer` or `worker`.
fn parse_role(raw: &str) -> Result<PeerRole, String> {
    match raw {
        "viewer" => Ok(PeerRole::Viewer),
        "worker" => Ok(PeerRole::Worker),
        other => Err(format!(
            "--role expects \"viewer\" or \"worker\", got {other:?}"
        )),
    }
}

fn arg_at<'a>(args: &'a [String], i: usize, flag: &str) -> Result<&'a str, String> {
    args.get(i)
        .map(String::as_str)
        .ok_or_else(|| format!("{flag} requires a value"))
}

fn parse_u32(flag: &str, raw: &str) -> Result<u32, String> {
    raw.parse::<u32>()
        .map_err(|_| format!("{flag} expects a non-negative integer, got {raw:?}"))
}

fn parse_render(args: &[String]) -> Result<RenderArgs, String> {
    let mut scene = None;
    let mut out = None;
    let mut width = None;
    let mut height = None;
    let mut samples = None;
    let mut threads = 0usize;
    let mut compute_mode = None;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--scene" => {
                i += 1;
                scene = Some(PathBuf::from(arg_at(args, i, "--scene")?));
            }
            "--out" => {
                i += 1;
                out = Some(PathBuf::from(arg_at(args, i, "--out")?));
            }
            "--width" => {
                i += 1;
                width = Some(parse_u32("--width", arg_at(args, i, "--width")?)?);
            }
            "--height" => {
                i += 1;
                height = Some(parse_u32("--height", arg_at(args, i, "--height")?)?);
            }
            "--samples" => {
                i += 1;
                samples = Some(parse_u32("--samples", arg_at(args, i, "--samples")?)?);
            }
            "--threads" => {
                i += 1;
                threads = parse_u32("--threads", arg_at(args, i, "--threads")?)? as usize;
            }
            "--only-gpu" => parse_compute_mode_flag(ComputeMode::OnlyGpu, &mut compute_mode)?,
            "--only-cpu" => parse_compute_mode_flag(ComputeMode::OnlyCpu, &mut compute_mode)?,
            other => {
                return Err(format!(
                    "unknown flag {other:?} for \"render\" (see --help)"
                ));
            }
        }
        i += 1;
    }

    Ok(RenderArgs {
        scene: scene.ok_or_else(|| "\"render\" requires --scene <path>".to_string())?,
        out: out.ok_or_else(|| "\"render\" requires --out <path>".to_string())?,
        width: width.ok_or_else(|| "\"render\" requires --width <pixels>".to_string())?,
        height: height.ok_or_else(|| "\"render\" requires --height <pixels>".to_string())?,
        samples: samples.ok_or_else(|| "\"render\" requires --samples <count>".to_string())?,
        threads,
        compute_mode: resolve_compute_mode(compute_mode)?,
    })
}

/// `serve`'s defaults: everything off, `--bind` [`super::DEFAULT_BIND`], 64 connections.
fn default_serve_args() -> ServeArgs {
    ServeArgs {
        bind: super::DEFAULT_BIND.to_string(),
        threads: 0,
        allow_remote: false,
        ca: None,
        cert: None,
        key: None,
        allowlist: None,
        trust_any_client_cert: false,
        insecure_no_tls: false,
        compute_mode: ComputeMode::default(),
        render: false,
        enroll_bind: None,
        no_enroll: false,
        worker_bind: None,
        worker_enroll_bind: None,
        no_workers: false,
        worker_allowlist: None,
        db: None,
        max_connections: super::DEFAULT_MAX_CONNECTIONS,
        max_preauth_per_ip: super::DEFAULT_MAX_PREAUTH_PER_IP,
        interactive_workers: super::ALL_INTERACTIVE_WORKERS,
        pin_interactive_worker: None,
        max_job_memory_mib: super::DEFAULT_MAX_JOB_MEMORY_MIB,
        whole_image_secs: super::DEFAULT_WHOLE_IMAGE_SECS,
        whole_image_pixel_samples: super::DEFAULT_WHOLE_IMAGE_PIXEL_SAMPLES,
        jobs_per_viewer: super::DEFAULT_JOBS_PER_VIEWER,
    }
}

fn parse_serve(args: &[String]) -> Result<ServeArgs, String> {
    let mut out = default_serve_args();
    let mut compute_mode = None;
    let mut explicit_render = false;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--render" => explicit_render = true,
            "--only-gpu" => parse_compute_mode_flag(ComputeMode::OnlyGpu, &mut compute_mode)?,
            "--only-cpu" => parse_compute_mode_flag(ComputeMode::OnlyCpu, &mut compute_mode)?,
            flag => i = parse_serve_flag(&mut out, args, i, flag)?,
        }
        i += 1;
    }

    if out.max_connections == 0 {
        return Err(
            "--max-connections must be at least 1 -- 0 would refuse every connection (see --help)"
                .to_string(),
        );
    }
    // `--only-gpu`/`--only-cpu` select the engine of the coordinator's own lane, so
    // either one implies `--render`.
    out.render = explicit_render || compute_mode.is_some();
    if out.render && !cfg!(feature = "worker") {
        return Err(
            "--render (and --only-gpu/--only-cpu, which imply it) needs a binary built with the \
             `worker` feature -- this build serves the design library only (see --help)"
                .to_string(),
        );
    }
    out.compute_mode = resolve_compute_mode(compute_mode)?;
    Ok(out)
}

/// One `serve` flag other than the compute-mode switches: applies it to `out` and
/// returns the index of the last argument it consumed.
fn parse_serve_flag(
    out: &mut ServeArgs,
    args: &[String],
    mut i: usize,
    flag: &str,
) -> Result<usize, String> {
    let value = |i: &mut usize| -> Result<String, String> {
        *i += 1;
        arg_at(args, *i, flag).map(str::to_string)
    };
    match flag {
        "--bind" => out.bind = value(&mut i)?,
        "--threads" => out.threads = parse_u32(flag, &value(&mut i)?)? as usize,
        "--allow-remote" => out.allow_remote = true,
        "--ca" => out.ca = Some(PathBuf::from(value(&mut i)?)),
        "--cert" => out.cert = Some(PathBuf::from(value(&mut i)?)),
        "--key" => out.key = Some(PathBuf::from(value(&mut i)?)),
        "--allowlist" => out.allowlist = Some(PathBuf::from(value(&mut i)?)),
        "--trust-any-client-cert" => out.trust_any_client_cert = true,
        "--insecure-no-tls" => out.insecure_no_tls = true,
        "--enroll-bind" => out.enroll_bind = Some(value(&mut i)?),
        "--no-enroll" => out.no_enroll = true,
        "--worker-bind" => out.worker_bind = Some(value(&mut i)?),
        "--worker-enroll-bind" => out.worker_enroll_bind = Some(value(&mut i)?),
        "--no-workers" => out.no_workers = true,
        "--worker-allowlist" => out.worker_allowlist = Some(PathBuf::from(value(&mut i)?)),
        "--db" => out.db = Some(PathBuf::from(value(&mut i)?)),
        "--max-connections" => {
            let max_connections = parse_u32(flag, &value(&mut i)?)?;
            if max_connections == 0 {
                return Err(
                    "--max-connections must be positive (0 would accept no connections at all)"
                        .to_string(),
                );
            }
            out.max_connections = max_connections as usize;
        }
        "--max-preauth-per-ip" => {
            let max = parse_u32(flag, &value(&mut i)?)?;
            if max == 0 {
                return Err(
                    "--max-preauth-per-ip must be positive (0 would refuse every connection)"
                        .to_string(),
                );
            }
            out.max_preauth_per_ip = max as usize;
        }
        "--interactive-workers" => {
            out.interactive_workers = parse_interactive_workers(&value(&mut i)?)?;
        }
        "--pin-interactive-worker" => {
            out.pin_interactive_worker = Some(parse_worker_label(&value(&mut i)?)?);
        }
        "--max-job-memory-mib" => out.max_job_memory_mib = parse_u32(flag, &value(&mut i)?)?,
        "--whole-image-secs" => out.whole_image_secs = parse_whole_image_secs(&value(&mut i)?)?,
        "--whole-image-pixel-samples" => {
            let raw = value(&mut i)?;
            out.whole_image_pixel_samples = raw
                .parse::<u64>()
                .map_err(|_| format!("{flag} expects a non-negative integer, got {raw:?}"))?;
        }
        "--jobs-per-viewer" => {
            let jobs = parse_u32(flag, &value(&mut i)?)?;
            if jobs == 0 {
                return Err(
                    "--jobs-per-viewer must be positive (0 would run no whole-image job at all)"
                        .to_string(),
                );
            }
            out.jobs_per_viewer = jobs;
        }
        other => return Err(format!("unknown flag {other:?} for \"serve\" (see --help)")),
    }
    Ok(i)
}

/// `--interactive-workers`'s value: `all` ([`super::ALL_INTERACTIVE_WORKERS`]) or a count.
fn parse_interactive_workers(raw: &str) -> Result<u32, String> {
    if raw == "all" {
        return Ok(super::ALL_INTERACTIVE_WORKERS);
    }
    raw.parse::<u32>().map_err(|_| {
        format!("--interactive-workers expects \"all\" or a non-negative integer, got {raw:?}")
    })
}

/// `--whole-image-secs`'s value: a finite, non-negative number of seconds. `0` is never
/// satisfied, so with `--whole-image-pixel-samples 0` as well no request is rendered whole.
fn parse_whole_image_secs(raw: &str) -> Result<f64, String> {
    match raw.parse::<f64>() {
        Ok(secs) if secs.is_finite() && secs >= 0.0 => Ok(secs),
        _ => Err(format!(
            "--whole-image-secs expects a non-negative number of seconds, got {raw:?}"
        )),
    }
}

/// `--pin-interactive-worker`'s value: a worker certificate label, given bare (`gpu-box`)
/// or as the certificate's full Common Name (`worker:gpu-box`).
fn parse_worker_label(value: &str) -> Result<String, String> {
    let label = value
        .strip_prefix(crate::pki::WORKER_CN_PREFIX)
        .unwrap_or(value);
    if label.is_empty() {
        return Err(
            "--pin-interactive-worker needs a worker certificate label (the --name the worker \
             certificate was issued with)"
                .to_string(),
        );
    }
    Ok(label.to_string())
}

fn parse_join(args: &[String]) -> Result<JoinArgs, String> {
    let mut out = JoinArgs {
        coordinator: String::new(),
        cert_dir: PathBuf::from(super::DEFAULT_JOIN_CERT_DIR),
        slots: super::DEFAULT_JOIN_SLOTS,
        threads: 0,
        compute_mode: ComputeMode::default(),
        token: None,
        enroll_addr: None,
    };
    let mut coordinator: Option<String> = None;
    let mut compute_mode = None;

    let mut i = 0;
    while i < args.len() {
        let flag = args[i].as_str();
        let value = |i: &mut usize| -> Result<String, String> {
            *i += 1;
            arg_at(args, *i, flag).map(str::to_string)
        };
        match flag {
            "--coordinator" => set_coordinator(&mut coordinator, value(&mut i)?)?,
            "--cert-dir" => out.cert_dir = PathBuf::from(value(&mut i)?),
            "--slots" => out.slots = parse_u32(flag, &value(&mut i)?)? as usize,
            "--threads" => out.threads = parse_u32(flag, &value(&mut i)?)? as usize,
            "--token" => out.token = Some(value(&mut i)?),
            "--enroll-addr" => out.enroll_addr = Some(value(&mut i)?),
            "--only-gpu" => parse_compute_mode_flag(ComputeMode::OnlyGpu, &mut compute_mode)?,
            "--only-cpu" => parse_compute_mode_flag(ComputeMode::OnlyCpu, &mut compute_mode)?,
            positional if !positional.starts_with('-') => {
                set_coordinator(&mut coordinator, positional.to_string())?;
            }
            other => return Err(format!("unknown flag {other:?} for \"join\" (see --help)")),
        }
        i += 1;
    }

    if !(1..=super::MAX_JOIN_SLOTS).contains(&out.slots) {
        return Err(format!(
            "--slots must be between 1 and {} (see --help)",
            super::MAX_JOIN_SLOTS
        ));
    }
    out.coordinator = coordinator.ok_or_else(|| {
        "\"join\" requires the coordinator's worker address (<host:port>, or --coordinator <host:port>)"
            .to_string()
    })?;
    if !cfg!(feature = "worker") {
        return Err(
            "\"join\" needs a binary built with the `worker` feature -- this build has no render \
             capacity to contribute (see --help)"
                .to_string(),
        );
    }
    out.compute_mode = resolve_compute_mode(compute_mode)?;
    Ok(out)
}

/// Records the coordinator address, refusing a second one (positional and
/// `--coordinator` together, or two positionals).
fn set_coordinator(slot: &mut Option<String>, addr: String) -> Result<(), String> {
    if slot.is_some() {
        return Err(
            "\"join\" takes exactly one coordinator address (positional or --coordinator)"
                .to_string(),
        );
    }
    *slot = Some(addr);
    Ok(())
}
