// No console window in a release build. Without this, launching the `.exe` directly
// (from Explorer, a shortcut, or the Start menu) opens an empty console alongside the
// window, because Rust binaries default to the Windows CONSOLE subsystem.
//
// `not(debug_assertions)` rather than unconditional: a debug build keeps its console so
// `cargo run` still shows panics and any `tracing` output during development. A release
// build's stderr goes nowhere with no console attached -- `install_tracing_subscriber`
// below also writes to a FILE for exactly that reason, the same "beside the exe, or
// temp dir" pattern `install_panic_logger` already uses.
//
// A no-op on every non-Windows target: the attribute is Windows-specific and simply
// ignored elsewhere.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

/// Installs a `tracing` subscriber writing to stderr AND a log file. Without one, every
/// `tracing::warn!`/`error!`/`debug!` call anywhere in this crate or `indicatrix` --
/// including the GPU backend's own "device lost"/"uncaptured wgpu error" diagnostics --
/// goes nowhere, and `RUST_LOG` has no effect whatsoever: a background-thread failure
/// would be undiagnosable from this app's own logs, since there wouldn't be any.
///
/// Default filter (overridden by `RUST_LOG` when set, exactly like
/// `apps/indicatrix-worker`'s own subscriber): `warn` everywhere, `info` for
/// `indicatrix`/`indicatrix_cut` specifically -- enough to see a device loss, a
/// self-heal, or a caught display-thread panic without paying for a per-sample or
/// per-frame log line anywhere on the hot path (none of this crate's `tracing` calls
/// are per-frame; the busiest is per accumulation-turn, already rare).
///
/// The file candidates mirror `install_panic_logger`'s exactly: next to the executable
/// first (findable from a shortcut or an unzipped folder), falling back to the temp
/// directory when that location is not writable. Missing/unwritable log locations are
/// not fatal: `try_init` (not `init`) and a missing file both just mean stderr-only (or,
/// worst case, no subscriber at all) rather than refusing to start the app over
/// diagnostics infrastructure.
fn install_tracing_subscriber() {
    use tracing_subscriber::{EnvFilter, fmt, layer::SubscriberExt, util::SubscriberInitExt as _};

    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("warn,indicatrix=info,indicatrix_cut=info"));

    let file_name = "indicatrix-cut.log";
    let beside_exe = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join(file_name)));
    let candidates = [beside_exe, Some(std::env::temp_dir().join(file_name))];
    let log_file = candidates.into_iter().flatten().find_map(|path| {
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .ok()
    });

    let stderr_layer = fmt::layer().with_writer(std::io::stderr).with_target(true);
    // `with_ansi(false)`: colour escape codes have no business in a plain-text log
    // file a user might open in Notepad.
    let file_layer = log_file.map(|file| {
        fmt::layer()
            .with_writer(file)
            .with_ansi(false)
            .with_target(true)
    });

    let _ = tracing_subscriber::registry()
        .with(filter)
        .with(stderr_layer)
        .with(file_layer)
        .try_init();
}

/// Writes panics to a file, because in a release build there is nowhere else for them
/// to go.
///
/// `windows_subsystem = "windows"` above means a release build has no console, and a
/// panic is not a `tracing` event (`install_tracing_subscriber`'s subscriber never sees
/// one) -- so without this, a panic on a BACKGROUND thread (the render thread, an
/// export worker, a batch lane, a remote-connection thread) would produce no output
/// whatsoever. The visible result is an app that keeps responding to the window
/// manager while some part of it has silently stopped, which is indistinguishable from
/// a hang and effectively undiagnosable from a bug report.
///
/// Chains to the previous hook rather than replacing it, so a debug build still prints
/// to the console it kept.
///
/// Failing to write the log is deliberately ignored: a panic handler that panics (or
/// that refuses to let the default handler run because a directory was read-only) would
/// be strictly worse than the silence it is here to fix.
fn install_panic_logger() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        // Next to the executable first -- this is the path a user can actually find
        // from a shortcut or an unzipped folder -- falling back to the temp directory
        // when that location is not writable (Program Files, a read-only mount).
        let file_name = "indicatrix-cut-panic.log";
        let beside_exe = std::env::current_exe()
            .ok()
            .and_then(|exe| exe.parent().map(|dir| dir.join(file_name)));
        let candidates = [beside_exe, Some(std::env::temp_dir().join(file_name))];

        let thread = std::thread::current();
        let entry = format!(
            "---\nthread: {}\n{info}\nbacktrace: {}\n",
            thread.name().unwrap_or("<unnamed>"),
            std::backtrace::Backtrace::force_capture()
        );

        for path in candidates.into_iter().flatten() {
            use std::io::Write as _;
            if let Ok(mut f) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)
                && f.write_all(entry.as_bytes()).is_ok()
            {
                break;
            }
        }

        previous(info);
    }));
}

fn main() -> anyhow::Result<()> {
    install_tracing_subscriber();
    install_panic_logger();
    indicatrix_cut::gui::main()
}
