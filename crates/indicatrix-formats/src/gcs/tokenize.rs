//! Splitting raw `.gcs` XML text into a flat stream of [`RawTag`]s, the substrate
//! [`super::parse::parse_gcs`]'s element-tree walk consumes.

use super::error::GcsParseError;

/// One `<tag ...>`, `<tag .../>`, or `</tag>` construct, with its attributes
/// already split out and entity-decoded. Owns its strings: `.gcs` files are tens
/// of KB, so the extra allocations are immaterial next to the clarity win.
pub(super) struct RawTag {
    pub(super) name: String,
    attrs: Vec<(String, String)>,
    pub(super) self_closing: bool,
    pub(super) closing: bool,
    /// 1-based line the tag started on.
    pub(super) line: usize,
}

impl RawTag {
    pub(super) fn attr(&self, key: &str) -> Option<&str> {
        self.attrs
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }

    /// Every attribute key, in source order.
    pub(super) fn keys(&self) -> impl Iterator<Item = &str> {
        self.attrs.iter().map(|(k, _)| k.as_str())
    }
}

/// Markup that is not an element: skipped whole. `(opening, closing)`.
const SKIPPED_MARKUP: [(&str, &str); 4] = [
    ("<!--", "-->"),
    ("<?", "?>"),
    ("<![CDATA[", "]]>"),
    ("<!", ">"),
];

/// Splits `content` into a flat stream of [`RawTag`]s.
///
/// Not a general XML tokenizer, but it accepts everything the published format
/// (Gem Cut Studio User's Manual v1.1.0 pp. 58-61) and standard XML put around
/// elements: an `<?xml ...?>` declaration, `<!-- -->` comments (the manual's own
/// example is full of them), `<!DOCTYPE ...>`, and single- or double-quoted
/// attribute values. A `>` or a raw newline inside a quoted value does not end the
/// tag (real `<info>` values carry raw CR/LF).
pub(super) fn tokenize(content: &str) -> Result<Vec<RawTag>, GcsParseError> {
    let bytes = content.as_bytes();
    let mut tags = Vec::new();
    let mut i = 0usize;
    let mut line = 1usize;
    while i < bytes.len() {
        if bytes[i] != b'<' {
            line += usize::from(bytes[i] == b'\n');
            i += 1;
            continue;
        }
        let tag_line = line;
        if let Some(end) = skip_markup(content, i, tag_line)? {
            line += count_newlines(&bytes[i..end]);
            i = end;
            continue;
        }
        let j =
            find_tag_end(bytes, i + 1).ok_or(GcsParseError::UnterminatedTag { line: tag_line })?;
        line += count_newlines(&bytes[i..j]);
        tags.push(split_tag(&content[i + 1..j], tag_line)?);
        i = j + 1;
    }
    Ok(tags)
}

/// When `content[start..]` opens a comment, declaration, CDATA or DOCTYPE, the
/// byte position just past its end; `None` for an ordinary tag.
fn skip_markup(content: &str, start: usize, line: usize) -> Result<Option<usize>, GcsParseError> {
    let rest = &content[start..];
    for (open, close) in SKIPPED_MARKUP {
        if let Some(body) = rest.strip_prefix(open) {
            return body
                .find(close)
                .map(|p| Some(start + open.len() + p + close.len()))
                .ok_or(GcsParseError::UnterminatedTag { line });
        }
    }
    Ok(None)
}

/// The position of the `>` that ends the tag whose body starts at `from`, honouring
/// single- and double-quoted attribute values.
fn find_tag_end(bytes: &[u8], from: usize) -> Option<usize> {
    let mut quote: Option<u8> = None;
    for (j, &b) in bytes.iter().enumerate().skip(from) {
        match (quote, b) {
            (None, b'"' | b'\'') => quote = Some(b),
            (Some(q), _) if b == q => quote = None,
            (None, b'>') => return Some(j),
            _ => {}
        }
    }
    None
}

fn count_newlines(bytes: &[u8]) -> usize {
    bytes.iter().fold(0, |n, &b| n + usize::from(b == b'\n'))
}

/// Splits one tag body (between `<` and `>`) into a [`RawTag`].
fn split_tag(inner: &str, line: usize) -> Result<RawTag, GcsParseError> {
    let closing = inner.starts_with('/');
    let self_closing = inner.trim_end().ends_with('/');
    let core = inner
        .strip_prefix('/')
        .unwrap_or(inner)
        .trim_end()
        .trim_end_matches('/')
        .trim();
    let (name, attr_str) = core
        .find(char::is_whitespace)
        .map_or((core, ""), |p| (&core[..p], core[p..].trim_start()));
    Ok(RawTag {
        name: name.to_string(),
        attrs: parse_attrs(attr_str, line)?,
        self_closing,
        closing,
        line,
    })
}

/// Parses `key="value"` / `key='value'` pairs out of one tag's attribute text,
/// decoding entities in each value. Order-independent.
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
        while i < bytes.len() && bytes[i] != b'"' && bytes[i] != b'\'' {
            i += 1;
        }
        let Some(&quote) = bytes.get(i) else {
            return Err(GcsParseError::MalformedAttribute { line, key });
        };
        i += 1;
        let val_start = i;
        while i < bytes.len() && bytes[i] != quote {
            i += 1;
        }
        if i >= bytes.len() {
            return Err(GcsParseError::MalformedAttribute { line, key });
        }
        let raw_value = &s[val_start..i];
        i += 1;
        out.push((key, decode_entities(raw_value)));
    }
    Ok(out)
}

/// The most characters between `&` and `;` that still name an entity: the longest
/// real body is `#x10FFFF`, 8 characters. Bounding the search for the `;` keeps
/// decoding linear in the input however many `&` it holds.
const MAX_ENTITY_BODY: usize = 12;

/// Decodes XML entities in one left-to-right pass, so `&amp;lt;` stays `&lt;`: the
/// five named entities and decimal/hex character references (`&#233;`, `&#xE9;`).
/// Anything else that starts with `&` (including one whose `;` is more than
/// [`MAX_ENTITY_BODY`] characters away) is kept verbatim.
pub(super) fn decode_entities(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        rest = &rest[amp..];
        // `rest` starts with `&`; `p` is the offset of a `;` within `rest[1..]`, so
        // the body is `rest[1..=p]` and the `;` itself sits at `rest[p + 1]`.
        let decoded = rest[1..]
            .char_indices()
            .take(MAX_ENTITY_BODY + 1)
            .find(|&(_, c)| c == ';')
            .and_then(|(p, _)| entity_char(&rest[1..=p]).map(|c| (c, p + 1)));
        if let Some((c, semi)) = decoded {
            out.push(c);
            rest = &rest[semi + 1..];
        } else {
            out.push('&');
            rest = &rest[1..];
        }
    }
    out.push_str(rest);
    out
}

/// The character an entity body (between `&` and `;`) stands for.
fn entity_char(body: &str) -> Option<char> {
    match body {
        "amp" => Some('&'),
        "lt" => Some('<'),
        "gt" => Some('>'),
        "quot" => Some('"'),
        "apos" => Some('\''),
        _ => {
            let digits = body.strip_prefix('#')?;
            let code = match digits.strip_prefix(['x', 'X']) {
                Some(hex) => u32::from_str_radix(hex, 16).ok()?,
                None => digits.parse().ok()?,
            };
            char::from_u32(code)
        }
    }
}
