//! A small block parser for the bundled user manual.
//!
//! The manual is plain Markdown with a handful of constructs: headings, paragraphs,
//! nested bullet and numbered lists, tables, fenced code, block quotes, the odd image and
//! HTML comment. [`parse_chapter`] splits one chapter into [`Block`]s and [`Section`]s.
//! Inline formatting (bold, links, code) stays in a block's `text` for the viewer to
//! render; [`plain_text`] strips it where plain words are needed (titles, search).
//!
//! Hard-wrapped source lines are joined with a space, so a paragraph is one block however
//! the file wraps it.

use std::{collections::HashMap, ops::Range};

/// The kind of a [`Block`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BlockKind {
    /// A paragraph of running text.
    #[default]
    Paragraph,
    /// A heading; `level` is 1 to 6 and `text` is plain.
    Heading,
    /// One list item; `marker` is its bullet or number, `indent` its nesting depth.
    ListItem,
    /// A block quote.
    Quote,
    /// A fenced code block; `text` holds the lines as written.
    Code,
    /// One row of a table; `cells` and `weights` hold the columns.
    TableRow,
    /// An image: `text` is its alt text and `marker` its path.
    Figure,
    /// A horizontal rule.
    Rule,
}

impl BlockKind {
    /// The number the Slint side switches on (`HelpBlock.kind` in `ui/models/help.slint`).
    #[must_use]
    pub const fn code(self) -> i32 {
        match self {
            Self::Paragraph => 0,
            Self::Heading => 1,
            Self::ListItem => 2,
            Self::Quote => 3,
            Self::Code => 4,
            Self::TableRow => 5,
            Self::Figure => 6,
            Self::Rule => 7,
        }
    }
}

/// One block of a chapter.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Block {
    /// What the block is.
    pub kind: BlockKind,
    /// The heading level (1 to 6); `0` for every other kind.
    pub level: u8,
    /// How many list levels the block sits inside (`0` at the top level). A list item's
    /// own depth, or that of the list item a paragraph, quote or code block belongs to.
    pub indent: u8,
    /// A list item's bullet or number; a figure's image path.
    pub marker: String,
    /// The block's text: inline Markdown for paragraphs, list items and quotes; plain text
    /// for headings, code and a figure's alt text.
    pub text: String,
    /// The cells of a table row, as inline Markdown.
    pub cells: Vec<String>,
    /// Whether a table row is the table's header row.
    pub header: bool,
    /// The relative width of each table column; the same on every row of one table.
    pub weights: Vec<f32>,
    /// The index of the section the block belongs to.
    pub section: usize,
}

impl Block {
    fn new(kind: BlockKind) -> Self {
        Self {
            kind,
            ..Self::default()
        }
    }

    /// The block's words without any formatting marks, for search and snippets.
    #[must_use]
    pub fn plain_text(&self) -> String {
        match self.kind {
            BlockKind::Heading | BlockKind::Code => self.text.clone(),
            BlockKind::Paragraph | BlockKind::ListItem | BlockKind::Quote | BlockKind::Figure => {
                plain_text(&self.text)
            }
            BlockKind::TableRow => self
                .cells
                .iter()
                .map(|cell| plain_text(cell))
                .collect::<Vec<_>>()
                .join(" | "),
            BlockKind::Rule => String::new(),
        }
    }
}

/// A part of a chapter that starts at a heading and runs to the next heading of any level.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Section {
    /// The heading level (1 to 6).
    pub level: u8,
    /// The heading, without formatting marks.
    pub title: String,
    /// The heading's anchor name, unique within the chapter (see [`slug`]).
    pub slug: String,
    /// The blocks of the section: the heading block first, then its text.
    pub range: Range<usize>,
}

/// One parsed chapter.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Chapter {
    /// The chapter's title (its first level-1 heading); empty when it has no heading.
    pub title: String,
    /// The sections in reading order. The first one is the chapter's own title.
    pub sections: Vec<Section>,
    /// The blocks in reading order.
    pub blocks: Vec<Block>,
}

impl Chapter {
    /// The index of the section whose anchor name is `slug`.
    #[must_use]
    pub fn section_by_slug(&self, slug: &str) -> Option<usize> {
        self.sections
            .iter()
            .position(|section| section.slug == slug)
    }

    /// The blocks that follow the heading of section `index` (its own text, not the
    /// text of the sections below it).
    #[must_use]
    pub fn section_body(&self, index: usize) -> &[Block] {
        let Some(blocks) = self
            .sections
            .get(index)
            .and_then(|section| self.blocks.get(section.range.clone()))
        else {
            return &[];
        };
        match blocks.split_first() {
            Some((first, rest)) if first.kind == BlockKind::Heading => rest,
            _ => blocks,
        }
    }

    /// The words of section `index` (its own text, blocks separated by a space).
    #[must_use]
    pub fn section_plain_text(&self, index: usize) -> String {
        self.section_body(index)
            .iter()
            .map(Block::plain_text)
            .filter(|text| !text.is_empty())
            .collect::<Vec<_>>()
            .join(" ")
    }
}

/// The anchor name of a heading.
///
/// The heading loses its formatting marks, goes lower case, has its spaces turned into
/// hyphens and every other mark dropped (the rule GitHub uses, near enough).
/// `## Targets: cut to depth` becomes `targets-cut-to-depth`.
#[must_use]
pub fn slug(heading: &str) -> String {
    plain_text(heading)
        .trim()
        .chars()
        .flat_map(char::to_lowercase)
        .filter_map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' {
                Some(c)
            } else if c.is_whitespace() {
                Some('-')
            } else {
                None
            }
        })
        .collect()
}

/// The text of `markdown` without its inline formatting: emphasis and code marks are
/// dropped (code keeps its content), a link or image keeps its label, an escaped mark
/// keeps the mark.
#[must_use]
pub fn plain_text(markdown: &str) -> String {
    let chars: Vec<char> = markdown.chars().collect();
    let mut out = String::with_capacity(markdown.len());
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let next = chars.get(i + 1).copied();
        if c == '\\'
            && let Some(mark) = next
            && mark.is_ascii_punctuation()
        {
            out.push(mark);
            i += 2;
        } else if c == '!'
            && next == Some('[')
            && let Some((label, end)) = link_at(&chars, i + 1)
        {
            out.push_str(&plain_text(&label));
            i = end;
        } else if c == '['
            && let Some((label, end)) = link_at(&chars, i)
        {
            out.push_str(&plain_text(&label));
            i = end;
        } else if c == '`' {
            if let Some(length) = chars[i + 1..].iter().position(|&other| other == '`') {
                out.extend(&chars[i + 1..i + 1 + length]);
                i += length + 2;
            } else {
                i += 1;
            }
        } else if c == '*' {
            i += 1;
        } else if c == '~' && next == Some('~') {
            i += 2;
        } else {
            out.push(c);
            i += 1;
        }
    }
    out
}

/// The label and the end (exclusive) of the link `[label](target)` whose `[` is at `open`.
fn link_at(chars: &[char], open: usize) -> Option<(String, usize)> {
    let close = open + 1 + chars.get(open + 1..)?.iter().position(|&c| c == ']')?;
    if chars.get(close + 1) != Some(&'(') {
        return None;
    }
    let end = close + 2 + chars.get(close + 2..)?.iter().position(|&c| c == ')')?;
    Some((chars[open + 1..close].iter().collect(), end + 1))
}

/// Parses one chapter's Markdown.
#[must_use]
pub fn parse_chapter(source: &str) -> Chapter {
    let mut parser = Parser {
        lines: source.lines().collect(),
        pos: 0,
        blocks: Vec::new(),
        sections: Vec::new(),
        items: Vec::new(),
        slugs: HashMap::new(),
    };
    parser.run();
    parser.finish()
}

struct Parser<'a> {
    lines: Vec<&'a str>,
    pos: usize,
    blocks: Vec<Block>,
    sections: Vec<Section>,
    /// The list items that are open at the current line: (marker column, text column).
    items: Vec<(usize, usize)>,
    /// How often each anchor name has been used, so a repeated heading gets `-1`, `-2`.
    slugs: HashMap<String, usize>,
}

impl Parser<'_> {
    fn run(&mut self) {
        while let Some(&line) = self.lines.get(self.pos) {
            if is_blank(line) {
                self.pos += 1;
            } else if line.trim_start().starts_with("<!--") {
                self.skip_comment();
            } else if let Some(fence) = fence_of(line) {
                self.code_block(fence);
            } else if let Some((level, text)) = heading_of(line) {
                self.heading(level, text);
                self.pos += 1;
            } else if is_rule(line) {
                let indent = self.block_indent(line);
                self.push(Block {
                    indent,
                    ..Block::new(BlockKind::Rule)
                });
                self.pos += 1;
            } else if table_start(&self.lines, self.pos) {
                self.table();
            } else if quote_text(line).is_some() {
                self.quote();
            } else if let Some(item) = list_line(line) {
                self.list_item(&item);
            } else if let Some((alt, path)) = figure_of(line) {
                let indent = self.block_indent(line);
                self.push(Block {
                    indent,
                    text: alt,
                    marker: path,
                    ..Block::new(BlockKind::Figure)
                });
                self.pos += 1;
            } else {
                self.paragraph();
            }
        }
    }

    fn finish(self) -> Chapter {
        let Self {
            blocks,
            mut sections,
            ..
        } = self;
        let starts: Vec<usize> = sections.iter().map(|section| section.range.start).collect();
        for (index, section) in sections.iter_mut().enumerate() {
            section.range.end = starts.get(index + 1).copied().unwrap_or(blocks.len());
        }
        // The shallowest heading names the chapter; the first one wins a tie.
        let title = sections
            .iter()
            .filter(|section| !section.title.is_empty())
            .min_by_key(|section| section.level)
            .map(|section| section.title.clone())
            .unwrap_or_default();
        Chapter {
            title,
            sections,
            blocks,
        }
    }

    /// Makes sure a section exists for text that comes before the first heading.
    fn ensure_section(&mut self) {
        if self.sections.is_empty() {
            let at = self.blocks.len();
            self.sections.push(Section {
                level: 1,
                title: String::new(),
                slug: String::new(),
                range: at..at,
            });
        }
    }

    fn push(&mut self, mut block: Block) {
        self.ensure_section();
        block.section = self.sections.len().saturating_sub(1);
        self.blocks.push(block);
    }

    fn unique_slug(&mut self, title: &str) -> String {
        let base = slug(title);
        let count = self.slugs.entry(base.clone()).or_insert(0);
        let unique = if *count == 0 {
            base
        } else {
            format!("{base}-{count}")
        };
        *count += 1;
        unique
    }

    fn heading(&mut self, level: u8, raw: &str) {
        self.items.clear();
        let title = plain_text(raw);
        let anchor = self.unique_slug(&title);
        let at = self.blocks.len();
        self.sections.push(Section {
            level,
            title: title.clone(),
            slug: anchor,
            range: at..at,
        });
        self.blocks.push(Block {
            level,
            text: title,
            section: self.sections.len() - 1,
            ..Block::new(BlockKind::Heading)
        });
    }

    /// The list depth a non-list block at `line`'s indentation belongs to; closes the
    /// list items it is no longer inside.
    fn block_indent(&mut self, line: &str) -> u8 {
        let column = indent_of(line);
        while self
            .items
            .last()
            .is_some_and(|&(_, content)| content > column)
        {
            self.items.pop();
        }
        u8::try_from(self.items.len()).unwrap_or(u8::MAX)
    }

    fn skip_comment(&mut self) {
        while let Some(line) = self.lines.get(self.pos) {
            self.pos += 1;
            if line.contains("-->") {
                break;
            }
        }
    }

    fn code_block(&mut self, fence: &str) {
        let open = self.lines[self.pos];
        let open_column = indent_of(open);
        let indent = self.block_indent(open);
        self.pos += 1;
        let mut code = Vec::new();
        while let Some(&line) = self.lines.get(self.pos) {
            self.pos += 1;
            if line.trim_start().starts_with(fence) {
                break;
            }
            code.push(strip_columns(line, open_column));
        }
        self.push(Block {
            indent,
            text: code.join("\n"),
            ..Block::new(BlockKind::Code)
        });
    }

    fn paragraph(&mut self) {
        let indent = self.block_indent(self.lines[self.pos]);
        let mut text = String::new();
        while let Some(&line) = self.lines.get(self.pos) {
            if !text.is_empty() && starts_block(&self.lines, self.pos, false) {
                break;
            }
            if !text.is_empty() {
                text.push(' ');
            }
            text.push_str(line.trim());
            self.pos += 1;
        }
        self.push(Block {
            indent,
            text,
            ..Block::new(BlockKind::Paragraph)
        });
    }

    fn list_item(&mut self, item: &ListLine) {
        while self
            .items
            .last()
            .is_some_and(|&(marker, _)| marker >= item.indent)
        {
            self.items.pop();
        }
        let depth = self.items.len();
        self.items.push((item.indent, item.content_indent));
        let mut text = item.text.trim().to_owned();
        self.pos += 1;
        while self.pos < self.lines.len() && !starts_block(&self.lines, self.pos, true) {
            text.push(' ');
            text.push_str(self.lines[self.pos].trim());
            self.pos += 1;
        }
        let marker = if item.ordered {
            item.marker.clone()
        } else if depth == 0 {
            "\u{2022}".to_owned()
        } else {
            "\u{2013}".to_owned()
        };
        self.push(Block {
            indent: u8::try_from(depth).unwrap_or(u8::MAX),
            marker,
            text,
            ..Block::new(BlockKind::ListItem)
        });
    }

    fn quote(&mut self) {
        let indent = self.block_indent(self.lines[self.pos]);
        let mut paragraphs = vec![String::new()];
        while let Some(rest) = self.lines.get(self.pos).and_then(|line| quote_text(line)) {
            let rest = rest.trim();
            if rest.is_empty() {
                if paragraphs.last().is_some_and(|last| !last.is_empty()) {
                    paragraphs.push(String::new());
                }
            } else if let Some(last) = paragraphs.last_mut() {
                if !last.is_empty() {
                    last.push(' ');
                }
                last.push_str(rest);
            }
            self.pos += 1;
        }
        for text in paragraphs.into_iter().filter(|text| !text.is_empty()) {
            self.push(Block {
                indent,
                text,
                ..Block::new(BlockKind::Quote)
            });
        }
    }

    fn table(&mut self) {
        let indent = self.block_indent(self.lines[self.pos]);
        let mut rows = vec![split_row(self.lines[self.pos])];
        self.pos += 2;
        while let Some(&line) = self.lines.get(self.pos) {
            if is_blank(line) || !line.contains('|') {
                break;
            }
            rows.push(split_row(line));
            self.pos += 1;
        }
        let columns = rows.first().map_or(0, Vec::len);
        for row in &mut rows {
            row.resize(columns, String::new());
        }
        let weights = column_weights(&rows, columns);
        for (index, cells) in rows.into_iter().enumerate() {
            self.push(Block {
                indent,
                cells,
                header: index == 0,
                weights: weights.clone(),
                ..Block::new(BlockKind::TableRow)
            });
        }
    }
}

/// The relative width of each of `columns` table columns: the average of the longest and
/// the mean cell, kept between 6 and 60 characters so no column vanishes or takes it all.
fn column_weights(rows: &[Vec<String>], columns: usize) -> Vec<f32> {
    (0..columns)
        .map(|column| {
            let lengths: Vec<usize> = rows
                .iter()
                .map(|row| {
                    row.get(column)
                        .map_or(0, |cell| plain_text(cell).chars().count())
                })
                .collect();
            let longest = lengths.iter().copied().max().unwrap_or(0) as f32;
            let mean = lengths.iter().sum::<usize>() as f32 / lengths.len().max(1) as f32;
            longest.midpoint(mean).clamp(6.0, 60.0)
        })
        .collect()
}

// --- Line classification ------------------------------------------------------------

fn is_blank(line: &str) -> bool {
    line.trim().is_empty()
}

/// The width of the line's leading whitespace in columns (a tab counts four).
fn indent_of(line: &str) -> usize {
    line.chars()
        .take_while(|c| c.is_whitespace())
        .map(|c| if c == '\t' { 4 } else { 1 })
        .sum()
}

/// `line` without its first `columns` columns of leading whitespace.
fn strip_columns(line: &str, columns: usize) -> String {
    let mut remaining = columns;
    let mut start = 0;
    for (at, c) in line.char_indices() {
        let width = match c {
            ' ' => 1,
            '\t' => 4,
            _ => break,
        };
        if remaining < width {
            break;
        }
        remaining -= width;
        start = at + c.len_utf8();
    }
    line[start..].trim_end().to_owned()
}

/// The fence (three backticks or three tildes) that opens a code block on `line`.
fn fence_of(line: &str) -> Option<&'static str> {
    let trimmed = line.trim_start();
    if trimmed.starts_with("```") {
        Some("```")
    } else if trimmed.starts_with("~~~") {
        Some("~~~")
    } else {
        None
    }
}

/// The level and text of an ATX heading (`## Text`).
fn heading_of(line: &str) -> Option<(u8, &str)> {
    if indent_of(line) > 3 {
        return None;
    }
    let trimmed = line.trim_start();
    let hashes = trimmed.chars().take_while(|&c| c == '#').count();
    let level = u8::try_from(hashes)
        .ok()
        .filter(|level| (1..=6).contains(level))?;
    let rest = &trimmed[hashes..];
    if !rest.is_empty() && !rest.starts_with([' ', '\t']) {
        return None;
    }
    Some((level, rest.trim()))
}

/// Whether `line` is a horizontal rule (`---`, `***`, `___`, spaces allowed between).
fn is_rule(line: &str) -> bool {
    if indent_of(line) > 3 {
        return false;
    }
    let compact: Vec<char> = line.chars().filter(|c| !c.is_whitespace()).collect();
    compact.len() >= 3
        && ['-', '*', '_']
            .iter()
            .any(|&mark| compact.iter().all(|&c| c == mark))
}

/// The text after the `>` of a block-quote line.
fn quote_text(line: &str) -> Option<&str> {
    let rest = line.trim_start().strip_prefix('>')?;
    Some(rest.strip_prefix(' ').unwrap_or(rest))
}

/// The alt text and path of a line that is one image, `![alt](path)`.
fn figure_of(line: &str) -> Option<(String, String)> {
    let rest = line.trim().strip_prefix("![")?;
    let (alt, tail) = rest.split_once("](")?;
    let path = tail.strip_suffix(')')?;
    Some((plain_text(alt), path.to_owned()))
}

/// A line that starts a list item.
struct ListLine<'a> {
    /// The column of the marker.
    indent: usize,
    /// The marker as written (`-`, `*`, `+`, `3.`, `3)`).
    marker: String,
    /// Whether the marker is a number.
    ordered: bool,
    /// The number of an ordered marker.
    number: Option<u64>,
    /// The column where the item's text starts.
    content_indent: usize,
    /// The item's first line of text.
    text: &'a str,
}

fn list_line(line: &str) -> Option<ListLine<'_>> {
    let indent = indent_of(line);
    let trimmed = line.trim_start();
    let first = trimmed.chars().next()?;
    let (marker_len, number) = if matches!(first, '-' | '*' | '+') {
        (1, None)
    } else if first.is_ascii_digit() {
        let digits = trimmed.chars().take_while(char::is_ascii_digit).count();
        let delimiter = trimmed[digits..].chars().next()?;
        if digits > 9 || !matches!(delimiter, '.' | ')') {
            return None;
        }
        (digits + 1, trimmed[..digits].parse::<u64>().ok())
    } else {
        return None;
    };
    let after = &trimmed[marker_len..];
    if !after.starts_with([' ', '\t']) {
        return None;
    }
    let text = after.trim_start();
    if text.is_empty() {
        return None;
    }
    let gap = (after.len() - text.len()).min(4);
    Some(ListLine {
        indent,
        marker: trimmed[..marker_len].to_owned(),
        ordered: number.is_some(),
        number,
        content_indent: indent + marker_len + gap,
        text,
    })
}

/// Whether the line at `at` starts a table: a row with a leading `|`, then the
/// `| --- | --- |` line.
fn table_start(lines: &[&str], at: usize) -> bool {
    lines
        .get(at)
        .is_some_and(|line| line.trim_start().starts_with('|'))
        && lines
            .get(at + 1)
            .is_some_and(|line| is_table_separator(line))
}

fn is_table_separator(line: &str) -> bool {
    let trimmed = line.trim();
    trimmed.contains('-')
        && trimmed.contains('|')
        && trimmed.chars().all(|c| matches!(c, '|' | '-' | ':' | ' '))
}

/// The cells of one table row. An escaped bar (`\|`) stays in its cell, escape and all, for
/// the inline renderer to resolve.
fn split_row(line: &str) -> Vec<String> {
    let trimmed = line.trim();
    let trimmed = trimmed.strip_prefix('|').unwrap_or(trimmed);
    let trimmed = trimmed.strip_suffix('|').unwrap_or(trimmed);
    let mut cells = Vec::new();
    let mut cell = String::new();
    let mut chars = trimmed.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\' && chars.peek() == Some(&'|') {
            cell.push_str("\\|");
            chars.next();
        } else if c == '|' {
            cells.push(cell.trim().to_owned());
            cell.clear();
        } else {
            cell.push(c);
        }
    }
    cells.push(cell.trim().to_owned());
    cells
}

/// Whether the line at `at` ends the paragraph or list item above it. `in_item` is true
/// while collecting the lines of a list item, where any list marker starts the next item;
/// a paragraph is only interrupted by a bullet or a list that starts at 1.
fn starts_block(lines: &[&str], at: usize, in_item: bool) -> bool {
    let Some(&line) = lines.get(at) else {
        return true;
    };
    if is_blank(line)
        || heading_of(line).is_some()
        || fence_of(line).is_some()
        || is_rule(line)
        || quote_text(line).is_some()
        || line.trim_start().starts_with("<!--")
        || table_start(lines, at)
    {
        return true;
    }
    list_line(line).is_some_and(|item| in_item || item.number.is_none_or(|number| number == 1))
}

#[cfg(test)]
mod tests;
