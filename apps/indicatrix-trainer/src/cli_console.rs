//! The command-line trainer's console: one live progress line at the bottom and messages
//! printed above it.
//!
//! On a terminal the progress line is redrawn in place. It is cut to the console width (read on
//! every redraw, so a resized window is followed) because a line that wraps cannot be redrawn:
//! `\r` and "erase line" only reach the last physical row, so every update of a wrapped line
//! left its first rows behind and the output scrolled. A message first erases the live line,
//! prints, and draws the line again, so messages never land behind the bar. When stdout is not
//! a terminal (a log file, a pipe), every progress line is printed in full on its own line.

use std::io::{IsTerminal, Write, stderr, stdout};

/// Width assumed when the console width cannot be read.
const FALLBACK_WIDTH: usize = 80;
/// Separator between the segments of the progress line.
const SEPARATOR: &str = " | ";

/// The console of a run: the live progress line and the messages above it.
#[derive(Debug)]
pub struct Console {
    /// `true` when stdout is a terminal, so the progress line is redrawn in place.
    live: bool,
    /// The segments of the progress line last shown on a live console.
    shown: Option<Vec<String>>,
}

impl Console {
    /// The console of this process.
    pub fn new() -> Self {
        Self {
            live: stdout().is_terminal(),
            shown: None,
        }
    }

    /// Shows the progress line built from `segments` (most important first).
    pub fn progress(&mut self, segments: &[String]) {
        if self.live {
            draw(segments);
            self.shown = Some(segments.to_vec());
        } else {
            println!("{}", segments.join(SEPARATOR));
        }
    }

    /// Prints `text` on stdout above the progress line.
    pub fn message(&self, text: &str) {
        self.erase();
        println!("{text}");
        self.redraw();
    }

    /// Prints `text` on stderr above the progress line.
    pub fn warning(&self, text: &str) {
        self.erase();
        eprintln!("{text}");
        let _ = stderr().flush();
        self.redraw();
    }

    /// Ends the live line, so what follows starts on a fresh line below the last progress.
    pub fn finish(&mut self) {
        if self.shown.take().is_some() {
            println!();
        }
    }

    /// Erases the live progress line, if one is shown.
    fn erase(&self) {
        if self.shown.is_some() {
            print!("\r\x1b[2K");
            let _ = stdout().flush();
        }
    }

    /// Draws the last progress line again.
    fn redraw(&self) {
        if let Some(segments) = &self.shown {
            draw(segments);
        }
    }
}

/// Draws `segments` on the live line, cut to the console width.
fn draw(segments: &[String]) {
    let width = terminal_size::terminal_size().map_or(FALLBACK_WIDTH, |(w, _)| usize::from(w.0));
    print!("\r\x1b[2K{}", fit(segments, width));
    let _ = stdout().flush();
}

/// Joins as many leading `segments` as fit in `width - 1` columns (the last column stays free:
/// some terminals wrap as soon as it is written). The first segment is always shown, cut if it
/// alone is too wide. Counts characters, which equals columns for the ASCII the trainer prints.
fn fit(segments: &[String], width: usize) -> String {
    let room = width.saturating_sub(1).max(1);
    let mut line = String::new();
    for segment in segments {
        let needed = if line.is_empty() {
            segment.chars().count()
        } else {
            SEPARATOR.len() + segment.chars().count()
        };
        if line.chars().count() + needed > room {
            if line.is_empty() {
                line = segment.chars().take(room).collect();
            }
            break;
        }
        if !line.is_empty() {
            line.push_str(SEPARATOR);
        }
        line.push_str(segment);
    }
    line
}

#[cfg(test)]
mod tests {
    use super::*;

    fn segments() -> Vec<String> {
        ["[####] 50.0%", "iter 5/10", "best 1.00", "reward [_.:#]"]
            .iter()
            .map(|s| (*s).to_owned())
            .collect()
    }

    #[test]
    fn a_wide_console_shows_every_segment() {
        assert_eq!(
            fit(&segments(), 200),
            "[####] 50.0% | iter 5/10 | best 1.00 | reward [_.:#]"
        );
    }

    #[test]
    fn a_narrow_console_drops_whole_segments_from_the_tail_and_never_wraps() {
        for width in 2..80 {
            let line = fit(&segments(), width);
            assert!(line.chars().count() < width, "{width}: {line}");
            assert!(!line.ends_with(SEPARATOR.trim_end()), "{width}: {line}");
        }
        assert_eq!(fit(&segments(), 25), "[####] 50.0% | iter 5/10");
        assert_eq!(fit(&segments(), 24), "[####] 50.0%");
    }

    #[test]
    fn the_first_segment_is_cut_when_it_alone_is_too_wide() {
        assert_eq!(fit(&segments(), 7), "[####]");
        assert_eq!(fit(&segments(), 0), "[");
    }
}
