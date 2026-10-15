//! The `indicatrix-cli` program: runs the command line given to it and exits with its code.
//!
//! Everything else is in the library (`indicatrix_cli`): this only collects the arguments,
//! names a newer release on standard error first (`indicatrix_cli::take_update_check`), prints
//! the text a command returned and turns its exit code into the process's.

use std::io::Write;

fn main() {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let skip = std::env::var_os(indicatrix_cli::NO_UPDATE_CHECK_ENV);
    if indicatrix_cli::take_update_check(&mut args, skip.as_deref()) {
        print_update_notice();
    }
    let outcome = indicatrix_cli::run(&args);
    // A closed pipe (`indicatrix-cli ... | head`) is not an error worth a message.
    let mut stdout = std::io::stdout().lock();
    let _ = stdout.write_all(outcome.stdout.as_bytes());
    let _ = stdout.flush();
    let _ = std::io::stderr().write_all(outcome.stderr.as_bytes());
    std::process::exit(outcome.exit);
}

/// Names a newer Indicatrix release in one `info:` line on standard error; prints nothing when
/// this is the latest release or GitHub cannot be asked (offline, timeout, rate limit).
fn print_update_notice() {
    if let Some(line) = indicatrix_update::cli_notice("indicatrix-cli", env!("CARGO_PKG_VERSION")) {
        eprintln!("info: {line}");
    }
}
