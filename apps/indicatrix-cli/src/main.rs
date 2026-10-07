//! The `indicatrix-cli` program: runs the command line given to it and exits with its code.
//!
//! Everything else is in the library (`indicatrix_cli`): this only collects the arguments,
//! prints the text a command returned and turns its exit code into the process's.

use std::io::Write;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let outcome = indicatrix_cli::run(&args);
    // A closed pipe (`indicatrix-cli ... | head`) is not an error worth a message.
    let mut stdout = std::io::stdout().lock();
    let _ = stdout.write_all(outcome.stdout.as_bytes());
    let _ = stdout.flush();
    let _ = std::io::stderr().write_all(outcome.stderr.as_bytes());
    std::process::exit(outcome.exit);
}
