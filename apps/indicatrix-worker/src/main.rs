//! `indicatrix-worker` CLI entry point. See `lib.rs` for the crate-level design notes and
//! `cli::HelpTopic`/its `USAGE_*` constants for full command-line help.

use indicatrix_worker::cli::{self, Command, HelpTopic};
use tracing_subscriber::{EnvFilter, fmt};

/// The per-user cache folder of the platform: `%LOCALAPPDATA%`, `~/Library/Caches`,
/// `$XDG_CACHE_HOME` or `~/.cache`. `None` when none of those variables is set. The worker
/// has no data-directory flag of its own, so the platform default is the only source.
#[cfg(feature = "gpu")]
fn platform_cache_dir() -> Option<std::path::PathBuf> {
    use std::path::PathBuf;
    if cfg!(target_os = "windows") {
        std::env::var_os("LOCALAPPDATA").map(PathBuf::from)
    } else if cfg!(target_os = "macos") {
        std::env::var_os("HOME").map(|home| PathBuf::from(home).join("Library").join("Caches"))
    } else if let Some(xdg) = std::env::var_os("XDG_CACHE_HOME") {
        Some(PathBuf::from(xdg))
    } else {
        std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cache"))
    }
}

/// Points the GPU renderer's driver pipeline cache at `<cache>/indicatrix/pipeline_cache`
/// (review item GPU-1): without it every start recompiles the megakernel before the first
/// frame. Must run before the first renderer is created; a no-op without a cache folder.
#[cfg(feature = "gpu")]
fn enable_gpu_pipeline_cache() {
    let Some(base) = platform_cache_dir() else {
        return;
    };
    let dir = base.join("indicatrix").join("pipeline_cache");
    if indicatrix::renderer::gpu::pipeline_cache::set_pipeline_cache_dir(dir.clone()) {
        tracing::info!(dir = %dir.display(), "GPU pipeline cache directory set");
    }
}

fn main() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let _ = fmt().with_env_filter(filter).with_target(true).try_init();
    #[cfg(feature = "gpu")]
    enable_gpu_pipeline_cache();

    let mut argv: Vec<String> = std::env::args().skip(1).collect();
    // The first log line names a newer release, if any (one GitHub request, at most 2 s);
    // `--no-update-check` (removed here, before parsing) or INDICATRIX_NO_UPDATE_CHECK skip it.
    let skip = std::env::var_os(indicatrix_update::NO_UPDATE_CHECK_ENV);
    if indicatrix_update::take_update_check(&mut argv, skip.as_deref())
        && let Some(line) =
            indicatrix_update::cli_notice("indicatrix-worker", env!("CARGO_PKG_VERSION"))
    {
        tracing::info!("{line}");
    }

    match cli::parse(&argv) {
        Ok(Command::Help(topic)) => print!("{}", topic.usage()),
        #[cfg(feature = "worker")]
        Ok(Command::Render(render_args)) => {
            if let Err(e) = indicatrix_worker::render_cmd::run(&render_args) {
                eprintln!("error: {e}");
                std::process::exit(1);
            }
        }
        #[cfg(not(feature = "worker"))]
        Ok(Command::Render(_)) => {
            eprintln!(
                "error: this build has no render capacity -- rebuild with `--features worker` \
                 (see --help)"
            );
            std::process::exit(1);
        }
        Ok(Command::Serve(serve_args)) => {
            if let Err(e) = indicatrix_worker::serve::run(&serve_args) {
                eprintln!("error: {e}");
                std::process::exit(1);
            }
        }
        #[cfg(feature = "worker")]
        Ok(Command::Join(join_args)) => {
            if let Err(e) = indicatrix_worker::join::run(&join_args) {
                eprintln!("error: {e}");
                std::process::exit(1);
            }
        }
        // Unreachable in practice: `cli::parse` already refuses `join` on this build.
        #[cfg(not(feature = "worker"))]
        Ok(Command::Join(_)) => {
            eprintln!(
                "error: this build has no render capacity to contribute -- rebuild with `--features worker` (see --help)"
            );
            std::process::exit(1);
        }
        Ok(Command::CertInit(init_args)) => {
            if let Err(e) = indicatrix_worker::pki::init(&init_args.dir) {
                eprintln!("error: {e}");
                std::process::exit(1);
            }
        }
        Ok(Command::CertIssueServer(server_args)) => {
            if let Err(e) = indicatrix_worker::pki::issue_server(
                &server_args.dir,
                &server_args.hosts,
                &server_args.ips,
            ) {
                eprintln!("error: {e}");
                std::process::exit(1);
            }
        }
        Ok(Command::CertIssueClient(client_args)) => {
            if let Err(e) = indicatrix_worker::pki::issue_client_with_role(
                &client_args.dir,
                &client_args.name,
                &client_args.out,
                client_args.role,
            ) {
                eprintln!("error: {e}");
                std::process::exit(1);
            }
        }
        Ok(Command::CertIssueToken(token_args)) => {
            if let Err(e) = indicatrix_worker::enroll_client::run_issue_token(&token_args) {
                eprintln!("error: {e}");
                std::process::exit(1);
            }
        }
        Ok(Command::CertClaim(claim_args)) => {
            if let Err(e) = indicatrix_worker::enroll_client::claim(&claim_args) {
                eprintln!("error: {e}");
                std::process::exit(1);
            }
        }
        Err(e) => {
            eprintln!("error: {e}\n");
            // Same topic resolution as a real --help, so a typo'd flag on
            // `render` prints render's own page rather than the whole document.
            eprint!("{}", HelpTopic::from_argv(&argv).usage());
            std::process::exit(2);
        }
    }
}
