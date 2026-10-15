//! Comment- and string-blanking scanner and statement splitter for the Slint sources.

/// A string literal found by [`scan`]; `at` indexes its opening quote in the masked text.
pub struct RawString {
    pub at: usize,
    pub line: usize,
    pub text: String,
}

/// A source with comments and string contents blanked (same length, same newlines) plus the
/// string literals that were blanked.
pub struct Scanned {
    pub masked: Vec<char>,
    pub strings: Vec<RawString>,
}

/// A blank of the same line structure: newlines stay, everything else becomes a space.
const fn blank(ch: char) -> char {
    if ch == '\n' { '\n' } else { ' ' }
}

/// Skips a `\{ ... }` interpolation inside a string; `i` is just after the `{`. The expression
/// is blanked in `masked` and not added to the string text.
fn skip_interpolation(
    chars: &[char],
    mut i: usize,
    line: &mut usize,
    masked: &mut Vec<char>,
) -> usize {
    let mut depth = 1;
    while i < chars.len() && depth > 0 {
        let ch = chars[i];
        match ch {
            '{' => depth += 1,
            '}' => depth -= 1,
            '"' => {
                masked.push(' ');
                i += 1;
                while i < chars.len() && chars[i] != '"' {
                    if chars[i] == '\n' {
                        *line += 1;
                    }
                    masked.push(blank(chars[i]));
                    i += 1;
                }
            }
            _ => {}
        }
        if ch == '\n' {
            *line += 1;
        }
        masked.push(blank(ch));
        i += 1;
    }
    i
}

/// Reads a string body starting just after its opening quote; returns the index after the
/// closing quote.
fn read_string(
    chars: &[char],
    mut i: usize,
    line: &mut usize,
    masked: &mut Vec<char>,
    text: &mut String,
) -> usize {
    while i < chars.len() {
        let ch = chars[i];
        if ch == '"' {
            masked.push('"');
            return i + 1;
        }
        if ch == '\n' {
            *line += 1;
        }
        text.push(ch);
        masked.push(blank(ch));
        i += 1;
        if ch == '\\' && i < chars.len() {
            let next = chars[i];
            if next == '\n' {
                *line += 1;
            }
            if next == '{' {
                masked.push(' ');
                i = skip_interpolation(chars, i + 1, line, masked);
            } else {
                text.push(next);
                masked.push(blank(next));
                i += 1;
            }
        }
    }
    i
}

/// Blanks comments and string contents of `source`.
pub fn scan(source: &str) -> Scanned {
    let chars: Vec<char> = source.chars().collect();
    let mut masked = Vec::with_capacity(chars.len());
    let mut strings = Vec::new();
    let (mut i, mut line) = (0, 1);
    while i < chars.len() {
        let ch = chars[i];
        let next = chars.get(i + 1).copied();
        if ch == '/' && next == Some('/') {
            while i < chars.len() && chars[i] != '\n' {
                masked.push(' ');
                i += 1;
            }
        } else if ch == '/' && next == Some('*') {
            masked.extend([' ', ' ']);
            i += 2;
            while i < chars.len() && !(chars[i] == '*' && chars.get(i + 1) == Some(&'/')) {
                if chars[i] == '\n' {
                    line += 1;
                }
                masked.push(blank(chars[i]));
                i += 1;
            }
            if i < chars.len() {
                masked.extend([' ', ' ']);
                i += 2;
            }
        } else if ch == '"' {
            masked.push('"');
            let (at, start_line) = (masked.len() - 1, line);
            let mut text = String::new();
            i = read_string(&chars, i + 1, &mut line, &mut masked, &mut text);
            strings.push(RawString {
                at,
                line: start_line,
                text,
            });
        } else {
            if ch == '\n' {
                line += 1;
            }
            masked.push(ch);
            i += 1;
        }
    }
    Scanned { masked, strings }
}

/// Kind of a [`Event`].
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Open,
    Close,
    Semi,
}

/// One statement boundary of the masked text: `text` is the statement before a `{` (Open), a `;`
/// or a closing `}` (Semi), or empty (Close); `line` is where the statement starts.
pub struct Event {
    pub kind: Kind,
    pub text: String,
    pub line: usize,
}

/// Splits the masked text into statement events.
pub fn events(masked: &[char]) -> Vec<Event> {
    let mut out = Vec::new();
    let (mut stmt, mut stmt_line, mut line) = (String::new(), 0, 1);
    let mut emit = |kind: Kind, stmt: &mut String, stmt_line: &mut usize, line: usize| {
        let text = std::mem::take(stmt);
        let at = if *stmt_line == 0 { line } else { *stmt_line };
        *stmt_line = 0;
        if kind != Kind::Close || !text.trim().is_empty() {
            let kind_for_text = if kind == Kind::Close {
                Kind::Semi
            } else {
                kind
            };
            out.push(Event {
                kind: kind_for_text,
                text,
                line: at,
            });
        }
        if kind == Kind::Close {
            out.push(Event {
                kind,
                text: String::new(),
                line,
            });
        }
    };
    for &ch in masked {
        match ch {
            '{' => emit(Kind::Open, &mut stmt, &mut stmt_line, line),
            ';' => emit(Kind::Semi, &mut stmt, &mut stmt_line, line),
            '}' => emit(Kind::Close, &mut stmt, &mut stmt_line, line),
            _ => {
                if ch == '\n' {
                    line += 1;
                }
                if stmt_line == 0 && !ch.is_whitespace() {
                    stmt_line = line;
                }
                stmt.push(ch);
            }
        }
    }
    out
}
