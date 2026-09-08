// No console window in a release build. Without this, launching the `.exe` directly
// (from Explorer, a shortcut, or the Start menu) opens an empty console alongside the
// window, because Rust binaries default to the Windows CONSOLE subsystem.
//
// `not(debug_assertions)` rather than unconditional: a debug build keeps its console so
// `cargo run` still shows panics and any `tracing` output during development. Note that
// this crate installs no `tracing` subscriber at all today, so a release build currently
// discards nothing by hiding the console -- but if one is ever added here, it must write
// somewhere other than stdout (a file, or the Windows event log) to survive this.
//
// A no-op on every non-Windows target: the attribute is Windows-specific and simply
// ignored elsewhere.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

/// Writes panics to a file, because in a release build there is nowhere else for them
/// to go.
///
/// `windows_subsystem = "windows"` above means a release build has no console, and this
/// crate installs no `tracing` subscriber -- so a panic on a BACKGROUND thread (the
/// render thread, an export worker, a batch lane, a remote-connection thread) currently
/// produces no output whatsoever. The visible result is an app that keeps responding to
/// the window manager while some part of it has silently stopped, which is
/// indistinguishable from a hang and effectively undiagnosable from a bug report.
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
    install_panic_logger();
    indicatrix_cut::gui::main()
}
