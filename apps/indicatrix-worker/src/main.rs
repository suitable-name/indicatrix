//! `indicatrix-worker` CLI entry point. See `lib.rs` for the crate-level design notes and
//! `cli::HelpTopic`/its `USAGE_*` constants for full command-line help.

use indicatrix_worker::cli::{self, Command, HelpTopic};
use tracing_subscriber::{EnvFilter, fmt};

fn main() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let _ = fmt().with_env_filter(filter).with_target(true).try_init();

    let argv: Vec<String> = std::env::args().skip(1).collect();

    match cli::parse(&argv) {
        Ok(Command::Help(topic)) => print!("{}", topic.usage()),
        #[cfg(feature = "worker")]
        Ok(Command::Render(args)) => {
            if let Err(e) = indicatrix_worker::render_cmd::run(&args) {
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
        Ok(Command::Serve(args)) => {
            if let Err(e) = indicatrix_worker::serve::run(&args) {
                eprintln!("error: {e}");
                std::process::exit(1);
            }
        }
        Ok(Command::CertInit(args)) => {
            if let Err(e) = indicatrix_worker::pki::init(&args.dir) {
                eprintln!("error: {e}");
                std::process::exit(1);
            }
        }
        Ok(Command::CertIssueServer(args)) => {
            if let Err(e) = indicatrix_worker::pki::issue_server(&args.dir, &args.hosts, &args.ips)
            {
                eprintln!("error: {e}");
                std::process::exit(1);
            }
        }
        Ok(Command::CertIssueClient(args)) => {
            if let Err(e) = indicatrix_worker::pki::issue_client(&args.dir, &args.name, &args.out) {
                eprintln!("error: {e}");
                std::process::exit(1);
            }
        }
        Ok(Command::CertIssueToken(args)) => {
            if let Err(e) = indicatrix_worker::enroll_client::run_issue_token(&args) {
                eprintln!("error: {e}");
                std::process::exit(1);
            }
        }
        Ok(Command::CertClaim(args)) => {
            if let Err(e) = indicatrix_worker::enroll_client::claim(&args) {
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
