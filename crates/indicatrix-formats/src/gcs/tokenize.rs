//! Splitting raw `.gcs` XML-ish text into a flat stream of [`RawTag`]s, the
//! substrate [`super::parse::parse_gcs`]'s element-tree walk consumes.

use super::error::GcsParseError;

/// One `<tag ...>`, `<tag .../>`, or `</tag>` construct, with its attributes
/// already split out. Owns its strings so the tokenizer's output can be consumed
/// by an ordinary `Vec` iterator without fighting borrow lifetimes against
/// `content` -- this format's files are small enough (tens of KB) that the extra
/// allocations are immaterial next to the clarity win.
pub(super) struct RawTag {
    pub(super) name: String,
    attrs: Vec<(String, String)>,
    pub(super) self_closing: bool,
    pub(super) closing: bool,
}

impl RawTag {
    pub(super) fn attr(&self, key: &str) -> Option<&str> {
        self.attrs
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }
}

/// Splits `content` into a flat stream of [`RawTag`]s. This is not a general XML
/// tokenizer: it only needs to respect double-quoted attribute values (so a `>`
/// inside a quoted value, or a literal newline inside one -- both real, seen in
/// the `<info>` element's multi-line `date`/`header2`/`header3`/`footer1`
/// attributes -- do not end the tag early), which is all `.gcs` files ever need.
pub(super) fn tokenize(content: &str) -> Result<Vec<RawTag>, GcsParseError> {
    let bytes = content.as_bytes();
    let mut tags = Vec::new();
    let mut i = 0usize;
    let mut line = 1usize;
    while i < bytes.len() {
        if bytes[i] != b'<' {
            if bytes[i] == b'\n' {
                line += 1;
            }
            i += 1;
            continue;
        }
        let tag_line = line;
        let mut j = i + 1;
        let mut in_quotes = false;
        while j < bytes.len() {
            match bytes[j] {
                b'"' => in_quotes = !in_quotes,
                b'\n' => line += 1,
                b'>' if !in_quotes => break,
                _ => {}
            }
            j += 1;
        }
        if j >= bytes.len() {
            return Err(GcsParseError::UnterminatedTag { line: tag_line });
        }
        let inner = &content[i + 1..j];
        let closing = inner.starts_with('/');
        let self_closing = inner.trim_end().ends_with('/');
        let core = inner
            .strip_prefix('/')
            .unwrap_or(inner)
            .trim_end_matches('/')
            .trim();
        let (name, attr_str) = core
            .find(char::is_whitespace)
            .map_or((core, ""), |p| (&core[..p], core[p..].trim_start()));
        tags.push(RawTag {
            name: name.to_string(),
            attrs: parse_attrs(attr_str, tag_line)?,
            self_closing,
            closing,
        });
        i = j + 1;
    }
    Ok(tags)
}

/// Parses `key="value"` pairs out of one tag's attribute text, decoding the five
/// standard XML entities in each value. Order-independent (see module docs: the
/// corpus's `<index>` attribute order happens to be consistent, but this crate
/// does not rely on that).
fn parse_attrs(s: &str, line: usize) -> Result<Vec<(String, String)>, GcsParseError> {
    let mut out = Vec::new();
    let bytes = s.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        if i >= bytes.len() {
            break;
        }
        let key_start = i;
        while i < bytes.len() && bytes[i] != b'=' && !bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        let key = s[key_start..i].to_string();
        while i < bytes.len() && bytes[i] != b'"' {
            i += 1;
        }
        if i >= bytes.len() {
            return Err(GcsParseError::MalformedAttribute { line, key });
        }
        i += 1; // opening quote
        let val_start = i;
        while i < bytes.len() && bytes[i] != b'"' {
            i += 1;
        }
        if i >= bytes.len() {
            return Err(GcsParseError::MalformedAttribute { line, key });
        }
        let raw_value = &s[val_start..i];
        i += 1; // closing quote
        out.push((key, decode_entities(raw_value)));
    }
    Ok(out)
}

/// Decodes the five standard XML entities. None appear in the sampled corpus, but
/// a real title or footnote containing `&` or `"` would need this, so it costs
/// nothing to handle correctly rather than assume it never happens.
fn decode_entities(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    s.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
}
