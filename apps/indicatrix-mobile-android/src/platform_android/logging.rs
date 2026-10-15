//! Logging on Android: everything ends up in logcat under the tag `indicatrix`.
//!
//! Three sources feed it: `log` records (through `android_logger`), Rust panics with a
//! backtrace (through `log-panics`, which needs the unwinding release profile), and the
//! `tracing` events of the shared crates (through a `fmt` subscriber whose writer
//! forwards each formatted line to `log`). Slint itself logs under the tag `slint`.
//!
//! Read it with `adb logcat -s indicatrix slint`.

use std::{io, sync::Once};

use tracing::{Level, Metadata};
use tracing_subscriber::{EnvFilter, fmt::MakeWriter};

/// Installs the logger, the panic hook and the `tracing` subscriber. Safe to call more
/// than once; only the first call has an effect.
pub(crate) fn init() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let debug_build = cfg!(debug_assertions);
        let level = if debug_build {
            log::LevelFilter::Debug
        } else {
            log::LevelFilter::Info
        };
        android_logger::init_once(
            android_logger::Config::default()
                .with_tag("indicatrix")
                .with_max_level(level),
        );

        // Release builds are stripped, so symbol names would be empty: addresses only.
        let backtrace = if debug_build {
            log_panics::BacktraceMode::Resolved
        } else {
            log_panics::BacktraceMode::Unresolved
        };
        log_panics::Config::new()
            .backtrace_mode(backtrace)
            .install_panic_hook();

        install_tracing();
    });
}

/// Routes `tracing` events (default filter `info`) into `log`.
///
/// `SubscriberBuilder::try_init` is deliberately not used: with the `tracing-log`
/// feature it would also install `LogTracer`, which fights `android_logger` for the
/// global `log` logger and would feed our own output back into `tracing`.
fn install_tracing() {
    let subscriber = tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::new("info"))
        .with_writer(LogMakeWriter)
        .with_ansi(false)
        .without_time()
        .finish();
    if tracing::subscriber::set_global_default(subscriber).is_err() {
        log::warn!(
            "a tracing subscriber was already installed; tracing output may not reach logcat"
        );
    }
}

/// Builds one [`LogLine`] per `tracing` event.
struct LogMakeWriter;

impl<'a> MakeWriter<'a> for LogMakeWriter {
    type Writer = LogLine;

    fn make_writer(&'a self) -> Self::Writer {
        LogLine::new(log::Level::Info)
    }

    fn make_writer_for(&'a self, meta: &Metadata<'_>) -> Self::Writer {
        LogLine::new(to_log_level(meta.level()))
    }
}

/// Maps a `tracing` level to the `log` level with the same meaning.
fn to_log_level(level: &Level) -> log::Level {
    if *level == Level::ERROR {
        log::Level::Error
    } else if *level == Level::WARN {
        log::Level::Warn
    } else if *level == Level::INFO {
        log::Level::Info
    } else if *level == Level::DEBUG {
        log::Level::Debug
    } else {
        log::Level::Trace
    }
}

/// One formatted event; it is written to `log` as a single record when dropped.
struct LogLine {
    level: log::Level,
    text: Vec<u8>,
}

impl LogLine {
    const fn new(level: log::Level) -> Self {
        Self {
            level,
            text: Vec::new(),
        }
    }
}

impl io::Write for LogLine {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        self.text.extend_from_slice(data);
        Ok(data.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Drop for LogLine {
    fn drop(&mut self) {
        let text = String::from_utf8_lossy(&self.text);
        let line = text.trim_end();
        if !line.is_empty() {
            log::log!(self.level, "{line}");
        }
    }
}
