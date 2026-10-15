//! Static checks over the Slint sources (`ui/**/*.slint`) and the Rust handlers (`src/**`).
//! No window is created and no extra dependency is used; the sources are read as text with a
//! small brace-aware scanner that blanks comments and string contents first.
//!
//! - `every_slint_callback_has_a_handler`: a declared `callback` that nothing handles is a silent
//!   no-op when the UI invokes it.
//! - `interactive_elements_have_labels`: an element with an interactive `accessible-role` must
//!   name itself for assistive technology.
//! - `user_facing_text_says_color`: user-facing strings say "color", never "colour".
//!
//! Every test prints its findings as `file:line: message` and fails when there are any.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};

#[path = "ui_static/scanner.rs"]
mod scanner;

use scanner::{Event, Kind, events, scan};

/// Callbacks (`Owner.name`) that are deliberately unhandled, each with the reason. Add an entry
/// only after reading the code that proves the callback is not needed; it starts empty.
/// (Callbacks with a Slint body, such as `TutorialsModel.open_browser` or `GuideModel.start`,
/// need no entry: the body counts as the handler.)
const CALLBACK_ALLOW_LIST: &[(&str, &str)] = &[];

/// `accessible-role` values that make an element interactive and so require a label.
const INTERACTIVE_ROLES: &[&str] = &[
    "button",
    "checkbox",
    "switch",
    "combobox",
    "slider",
    "spinbox",
    "tab",
    "text-input",
];

/// String properties that are never shown to the user, so a "colour" in them is not text.
const NON_DISPLAY_PROPERTIES: &[&str] = &["source", "font-family", "icon"];

// ---------------------------------------------------------------------------------------------
// File access
// ---------------------------------------------------------------------------------------------

/// The crate directory (`apps/indicatrix-cut`).
fn crate_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// Appends every file below `dir` with extension `ext` to `out`, in sorted order.
fn collect_files(dir: &Path, ext: &str, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let mut paths: Vec<PathBuf> = entries.filter_map(|e| e.ok().map(|e| e.path())).collect();
    paths.sort();
    for path in paths {
        if path.is_dir() {
            collect_files(&path, ext, out);
        } else if path.extension().is_some_and(|e| e == ext) {
            out.push(path);
        }
    }
}

/// Every `.slint` file under `ui/` as `(path relative to ui/, text)`.
fn slint_sources() -> Vec<(String, String)> {
    let ui = crate_dir().join("ui");
    let mut files = Vec::new();
    collect_files(&ui, "slint", &mut files);
    assert!(!files.is_empty(), "no .slint files under {}", ui.display());
    files
        .iter()
        .map(|path| {
            let rel = path
                .strip_prefix(&ui)
                .unwrap_or(path)
                .to_string_lossy()
                .replace('\\', "/");
            let text = fs::read_to_string(path)
                .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
            (rel, text)
        })
        .collect()
}

/// Adds every identifier that starts with `on_` in `text` to `names`.
fn collect_on_names(text: &str, names: &mut BTreeSet<String>) {
    let bytes = text.as_bytes();
    for (at, _) in text.match_indices("on_") {
        if at > 0 && (bytes[at - 1].is_ascii_alphanumeric() || bytes[at - 1] == b'_') {
            continue;
        }
        let end = bytes[at..]
            .iter()
            .position(|b| !(b.is_ascii_alphanumeric() || *b == b'_'))
            .map_or(bytes.len(), |n| at + n);
        names.insert(text[at..end].to_string());
    }
}

/// Every `on_*` identifier in the crate's Rust sources (`src/**`, test files left out).
fn rust_handler_names() -> BTreeSet<String> {
    let src = crate_dir().join("src");
    let mut files = Vec::new();
    collect_files(&src, "rs", &mut files);
    assert!(!files.is_empty(), "no .rs files under {}", src.display());
    let mut names = BTreeSet::new();
    for path in files {
        let rel = path.strip_prefix(&src).unwrap_or(&path);
        if rel
            .components()
            .any(|c| c.as_os_str().to_string_lossy().contains("test"))
        {
            continue;
        }
        if let Ok(text) = fs::read_to_string(&path) {
            collect_on_names(&text, &mut names);
        }
    }
    names
}

// ---------------------------------------------------------------------------------------------
// Check 1: callbacks
// ---------------------------------------------------------------------------------------------

/// What a `{` opened.
#[derive(Clone)]
enum Block {
    /// A `component` or `global` body.
    Component(String),
    /// An element (`Type { ... }`) inside a component.
    Element(String),
    /// A callback handler body.
    Handler,
    /// Anything else (functions, struct literals, `states`, `animate`, ...).
    Other,
}

/// A `callback` declaration.
struct Decl {
    owner: String,
    name: String,
    file: String,
    line: usize,
}

/// Everything the callback check learns from the Slint sources.
#[derive(Default)]
struct Model {
    decls: Vec<Decl>,
    /// `(type, callback)` pairs bound by a body or `<=>` on that type's root or an instance.
    bound: BTreeSet<(String, String)>,
    /// Component name to the component it inherits.
    bases: BTreeMap<String, String>,
    /// `(component, callback, child id, child callback)` for `callback a <=> child.b;`.
    aliases: Vec<(String, String, String, String)>,
    /// `(component, element id)` to the element's type.
    ids: BTreeMap<(String, String), String>,
}

fn is_ident_char(ch: char) -> bool {
    ch.is_alphanumeric() || ch == '_' || ch == '-'
}

/// `(name, base)` of a `[export] component|global Name [inherits Base]` header.
fn component_header(stmt: &str) -> Option<(String, Option<String>)> {
    let words: Vec<&str> = stmt.split_whitespace().collect();
    let at = words
        .iter()
        .position(|w| *w == "component" || *w == "global")?;
    let name = (*words.get(at + 1)?).to_string();
    let base = (words.get(at + 2) == Some(&"inherits"))
        .then(|| words.get(at + 3).map(|w| (*w).to_string()))
        .flatten();
    Some((name, base))
}

/// The type of an element statement (`Type`, `id := Type`, `if c: Type`, `for x in y: Type`).
fn element_type(stmt: &str) -> Option<String> {
    let s = stmt.trim();
    let tail = if let Some((_, after)) = s.split_once(":=") {
        after
    } else if s.starts_with("if ") || s.starts_with("for ") {
        s.rsplit_once(':')?.1
    } else {
        s
    };
    let word: String = tail
        .trim()
        .chars()
        .take_while(|c| is_ident_char(*c) || *c == '.')
        .collect();
    word.chars().next()?.is_uppercase().then_some(word)
}

/// The callback name of a handler header such as `clicked =>` or `edited(text) =>`.
fn handler_name(stmt: &str) -> Option<String> {
    let head = stmt.trim().trim_end_matches("=>").trim();
    let name: String = head.chars().take_while(|c| is_ident_char(*c)).collect();
    let rest = head[name.len()..].trim();
    (!name.is_empty() && (rest.is_empty() || rest.starts_with('('))).then_some(name)
}

/// `(name, forward target)` of a `[pure|public] callback name(...)... [<=> target];` statement.
fn parse_callback(stmt: &str) -> Option<(String, Option<String>)> {
    let mut s = stmt.trim();
    while let Some(rest) = s
        .strip_prefix("pure ")
        .or_else(|| s.strip_prefix("public "))
        .or_else(|| s.strip_prefix("private "))
    {
        s = rest.trim_start();
    }
    let rest = s.strip_prefix("callback ")?.trim();
    let name: String = rest.chars().take_while(|c| is_ident_char(*c)).collect();
    let forward = rest[name.len()..]
        .trim()
        .strip_prefix("<=>")
        .map(|t| t.trim().to_string());
    (!name.is_empty()).then_some((name, forward))
}

/// The component or element type that owns statements directly inside `block`.
fn block_type(block: Option<&Block>) -> Option<&str> {
    match block? {
        Block::Component(n) | Block::Element(n) => Some(n),
        _ => None,
    }
}

/// The nearest enclosing component in `stack`.
fn owner_of(stack: &[Block]) -> Option<&str> {
    stack.iter().rev().find_map(|b| match b {
        Block::Component(n) => Some(n.as_str()),
        _ => None,
    })
}

/// Classifies the block a `{` opens and records what it binds.
fn classify_open(stmt: &str, stack: &[Block], model: &mut Model) -> Block {
    let trimmed = stmt.trim();
    let parent_type = block_type(stack.last()).map(str::to_string);
    if trimmed.ends_with("=>") {
        if let (Some(ty), Some(name)) = (parent_type, handler_name(trimmed)) {
            model.bound.insert((ty, name));
        }
        return Block::Handler;
    }
    if stack.is_empty() {
        return match component_header(trimmed) {
            Some((name, base)) => {
                if let Some(base) = base {
                    model.bases.insert(name.clone(), base);
                }
                Block::Component(name)
            }
            None => Block::Other,
        };
    }
    if parent_type.is_none() {
        return Block::Other;
    }
    match element_type(trimmed) {
        Some(ty) => {
            if let (Some((id, _)), Some(owner)) = (trimmed.split_once(":="), owner_of(stack)) {
                model
                    .ids
                    .insert((owner.to_string(), id.trim().to_string()), ty.clone());
            }
            Block::Element(ty)
        }
        None => Block::Other,
    }
}

/// Records a `;`-terminated statement: callback declarations and `name <=> target` bindings.
fn record_statement(file: &str, ev: &Event, stack: &[Block], model: &mut Model) {
    let Some(ty) = block_type(stack.last()).map(str::to_string) else {
        return;
    };
    if let Some((name, forward)) = parse_callback(&ev.text) {
        let Some(Block::Component(owner)) = stack.last() else {
            return;
        };
        if let Some(target) = forward
            && let Some((id, bar)) = target.split_once('.')
        {
            model.aliases.push((
                owner.clone(),
                name.clone(),
                id.trim().to_string(),
                bar.trim().to_string(),
            ));
        }
        model.decls.push(Decl {
            owner: owner.clone(),
            name,
            file: file.to_string(),
            line: ev.line,
        });
    } else if let Some((left, _)) = ev.text.split_once("<=>") {
        let left = left.trim();
        if !left.is_empty() && left.chars().all(is_ident_char) {
            model.bound.insert((ty, left.to_string()));
        }
    }
}

/// Feeds one file into `model`.
fn analyse_file(file: &str, source: &str, model: &mut Model) {
    let scanned = scan(source);
    let mut stack: Vec<Block> = Vec::new();
    for ev in events(&scanned.masked) {
        match ev.kind {
            Kind::Open => {
                let block = classify_open(&ev.text, &stack, model);
                stack.push(block);
            }
            Kind::Close => {
                stack.pop();
            }
            Kind::Semi => record_statement(file, &ev, &stack, model),
        }
    }
}

/// Whether type `descendant` is `ancestor` or inherits from it (through custom components).
fn inherits(model: &Model, descendant: &str, ancestor: &str) -> bool {
    let mut current = descendant;
    for _ in 0..32 {
        if current == ancestor {
            return true;
        }
        match model.bases.get(current) {
            Some(base) => current = base,
            None => return false,
        }
    }
    false
}

/// Findings for callbacks with no Rust handler, Slint body or binding.
fn callback_findings(files: &[(String, String)], rust: &BTreeSet<String>) -> Vec<String> {
    let mut model = Model::default();
    for (rel, text) in files {
        analyse_file(rel, text, &mut model);
    }
    let mut satisfied: BTreeSet<(String, String)> = BTreeSet::new();
    for d in &model.decls {
        let rust_name = format!("on_{}", d.name.replace('-', "_"));
        let bound = model
            .bound
            .iter()
            .any(|(t, n)| *n == d.name && inherits(&model, t, &d.owner));
        if bound || rust.contains(&rust_name) {
            satisfied.insert((d.owner.clone(), d.name.clone()));
        }
    }
    loop {
        let mut changed = false;
        for (owner, name, id, bar) in &model.aliases {
            if !satisfied.contains(&(owner.clone(), name.clone())) {
                continue;
            }
            if let Some(ty) = model.ids.get(&(owner.clone(), id.clone())) {
                changed |= satisfied.insert((ty.clone(), bar.clone()));
            }
        }
        if !changed {
            break;
        }
    }
    model
        .decls
        .iter()
        .filter(|d| !satisfied.contains(&(d.owner.clone(), d.name.clone())))
        .filter(|d| {
            let key = format!("{}.{}", d.owner, d.name);
            !CALLBACK_ALLOW_LIST.iter().any(|(k, _)| *k == key)
        })
        .map(|d| {
            format!(
                "{}:{}: callback `{}.{}` has no Rust `on_{}` handler, Slint body or binding",
                d.file,
                d.line,
                d.owner,
                d.name,
                d.name.replace('-', "_")
            )
        })
        .collect()
}

#[test]
fn every_slint_callback_has_a_handler() {
    let files = slint_sources();
    let rust = rust_handler_names();
    let mut probe = Model::default();
    for (rel, text) in &files {
        analyse_file(rel, text, &mut probe);
    }
    assert!(
        probe.decls.len() >= 300,
        "only {} callback declarations were parsed: the scanner is broken",
        probe.decls.len()
    );
    let findings = callback_findings(&files, &rust);
    assert!(
        findings.is_empty(),
        "{} Slint callback(s) are never handled:\n{}",
        findings.len(),
        findings.join("\n")
    );
}

// ---------------------------------------------------------------------------------------------
// Check 2: accessible labels
// ---------------------------------------------------------------------------------------------

/// Every start index of `pat` in `chars`.
fn find_all(chars: &[char], pat: &str) -> Vec<usize> {
    let p: Vec<char> = pat.chars().collect();
    chars
        .windows(p.len())
        .enumerate()
        .filter(|(_, w)| *w == p.as_slice())
        .map(|(i, _)| i)
        .collect()
}

/// 1-based line of `idx`.
fn line_of(chars: &[char], idx: usize) -> usize {
    chars[..idx].iter().filter(|c| **c == '\n').count() + 1
}

/// Whether `pat` starts at `at` as a whole property name (not part of a longer identifier).
fn property_at(chars: &[char], at: usize, pat: &str) -> bool {
    let p: Vec<char> = pat.chars().collect();
    chars.get(at..at + p.len()) == Some(p.as_slice()) && (at == 0 || !is_ident_char(chars[at - 1]))
}

/// The role named after `accessible-role` at `at`, without the `AccessibleRole.` prefix.
fn role_after(chars: &[char], at: usize) -> Option<String> {
    let mut i = at + "accessible-role".len();
    while chars.get(i).is_some_and(|c| c.is_whitespace()) {
        i += 1;
    }
    if chars.get(i) != Some(&':') {
        return None;
    }
    i += 1;
    while chars.get(i).is_some_and(|c| c.is_whitespace()) {
        i += 1;
    }
    let word: String = chars[i..]
        .iter()
        .take_while(|c| is_ident_char(**c) || **c == '.')
        .collect();
    Some(
        word.strip_prefix("AccessibleRole.")
            .unwrap_or(&word)
            .to_string(),
    )
}

/// Whether the `accessible-label` at `at` is a binding (`:` or `<=>` follows).
fn label_binding_at(chars: &[char], at: usize) -> bool {
    if !property_at(chars, at, "accessible-label") {
        return false;
    }
    let mut i = at + "accessible-label".len();
    while chars.get(i).is_some_and(|c| c.is_whitespace()) {
        i += 1;
    }
    chars.get(i) == Some(&':') || chars.get(i..i + 3) == Some(['<', '=', '>'].as_slice())
}

/// The index of the `{` of the block that directly contains `at`.
fn enclosing_open(chars: &[char], at: usize) -> Option<usize> {
    let mut depth = 0;
    for j in (0..at).rev() {
        match chars[j] {
            '}' => depth += 1,
            '{' if depth == 0 => return Some(j),
            '{' => depth -= 1,
            _ => {}
        }
    }
    None
}

/// Whether the block opened at `open` sets `accessible-label` in its own body (not a child's).
fn has_direct_label(chars: &[char], open: usize) -> bool {
    let mut depth = 0;
    for k in open + 1..chars.len() {
        match chars[k] {
            '{' => depth += 1,
            '}' if depth == 0 => return false,
            '}' => depth -= 1,
            'a' if depth == 0 && label_binding_at(chars, k) => return true,
            _ => {}
        }
    }
    false
}

/// Findings for interactive-role elements without a label in one file.
fn label_findings(file: &str, source: &str) -> (usize, Vec<String>) {
    let chars = scan(source).masked;
    let (mut checked, mut findings) = (0, Vec::new());
    for at in find_all(&chars, "accessible-role") {
        if !property_at(&chars, at, "accessible-role") {
            continue;
        }
        let Some(role) = role_after(&chars, at) else {
            continue;
        };
        if !INTERACTIVE_ROLES.contains(&role.as_str()) {
            continue;
        }
        checked += 1;
        let labelled =
            enclosing_open(&chars, at).is_some_and(|open| has_direct_label(&chars, open));
        if !labelled {
            findings.push(format!(
                "{file}:{}: element with accessible-role `{role}` has no `accessible-label` in its block",
                line_of(&chars, at)
            ));
        }
    }
    (checked, findings)
}

#[test]
fn interactive_elements_have_labels() {
    let mut checked = 0;
    let mut findings = Vec::new();
    for (rel, text) in slint_sources() {
        let (n, f) = label_findings(&rel, &text);
        checked += n;
        findings.extend(f);
    }
    assert!(
        checked > 0,
        "no interactive accessible-role was found: the scanner is broken"
    );
    assert!(
        findings.is_empty(),
        "{} of {checked} interactive element(s) have no accessible-label:\n{}",
        findings.len(),
        findings.join("\n")
    );
}

// ---------------------------------------------------------------------------------------------
// Check 3: "color" spelling in user-facing strings
// ---------------------------------------------------------------------------------------------

/// The property a string literal belongs to (`text`, `title`, `@tr`, ...), from the statement
/// that holds it; `None` when it is an `import` or `export` path.
fn string_owner(masked: &[char], at: usize) -> Option<String> {
    let start = masked[..at]
        .iter()
        .rposition(|c| matches!(c, ';' | '{' | '}'))
        .map_or(0, |p| p + 1);
    let stmt: String = masked[start..at].iter().collect();
    let stmt = stmt.trim();
    // `import { A, B } from "x.slint"`: the search above stops at the `}` of the name list, so the
    // path's statement reads as just `from`.
    if stmt.starts_with("import") || stmt.starts_with("export") || stmt == "from" {
        return None;
    }
    let lead: String = stmt.chars().take_while(|c| is_ident_char(*c)).collect();
    if !lead.is_empty() && stmt[lead.len()..].trim_start().starts_with(':') {
        return Some(lead);
    }
    let tail: String = stmt
        .chars()
        .rev()
        .skip_while(|c| *c == '(' || c.is_whitespace())
        .take_while(|c| is_ident_char(*c) || *c == '@')
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    Some(if tail.is_empty() { "?".into() } else { tail })
}

/// Findings for "colour" in string literals of one file.
fn spelling_findings(file: &str, source: &str) -> Vec<String> {
    let scanned = scan(source);
    scanned
        .strings
        .iter()
        .filter(|s| s.text.to_lowercase().contains("colour"))
        .filter_map(|s| {
            let owner = string_owner(&scanned.masked, s.at)?;
            if NON_DISPLAY_PROPERTIES.contains(&owner.as_str()) {
                return None;
            }
            Some(format!(
                "{file}:{}: `{owner}` string \"{}\" says \"colour\"; write \"color\"",
                s.line,
                s.text.replace('\n', " ")
            ))
        })
        .collect()
}

#[test]
fn user_facing_text_says_color() {
    let findings: Vec<String> = slint_sources()
        .iter()
        .flat_map(|(rel, text)| spelling_findings(rel, text))
        .collect();
    assert!(
        findings.is_empty(),
        "{} user-facing string(s) spell \"colour\":\n{}",
        findings.len(),
        findings.join("\n")
    );
}

// ---------------------------------------------------------------------------------------------
// Scanner self-checks
// ---------------------------------------------------------------------------------------------

/// A source with comments and strings that must not confuse the scanner.
const SAMPLE: &str = r#"
import { RoughColourWindow, ImportSlot } from "rough_colour_window.slint";
// callback commented() ;
export global Demo {
    callback handled(string);
    callback orphan();
    callback with-body() -> int;
    with-body => { return 1; }
    callback forwarded <=> inner.value;
}
component Row inherits Rectangle {
    callback picked(int);
    callback never(); /* callback hidden(); */
    in-out property <string> text: "a { b ; c";
    inner := Leaf { }
    Button { accessible-role: button; accessible-label: "ok"; }
    Rectangle { accessible-role: AccessibleRole.checkbox; }
    Text { text: "Colour \{root.colour} here"; }
}
export component Leaf inherits Rectangle { callback value(); }
export component Page inherits Window {
    Row { picked(i) => { } }
}
"#;

#[test]
fn scanner_blanks_comments_and_strings() {
    let scanned = scan(SAMPLE);
    let masked: String = scanned.masked.iter().collect();
    assert!(!masked.contains("commented"));
    assert!(!masked.contains("hidden"));
    assert!(!masked.contains("a { b"));
    assert_eq!(masked.lines().count(), SAMPLE.lines().count());
    assert!(
        scanned
            .strings
            .iter()
            .any(|s| s.text == "Colour " || s.text.starts_with("Colour "))
    );
}

#[test]
fn callback_check_follows_bodies_bindings_and_forwarding() {
    let files = vec![("sample.slint".to_string(), SAMPLE.to_string())];
    let rust: BTreeSet<String> = ["on_handled".to_string()].into();
    let findings = callback_findings(&files, &rust).join("\n");
    assert!(findings.contains("Demo.orphan"), "{findings}");
    assert!(findings.contains("Row.never"), "{findings}");
    assert!(!findings.contains("Demo.handled"), "{findings}");
    assert!(!findings.contains("Demo.with-body"), "{findings}");
    assert!(!findings.contains("Row.picked"), "{findings}");
    assert!(!findings.contains("commented"), "{findings}");
}

#[test]
fn label_check_sees_the_block_of_each_role() {
    let (checked, findings) = label_findings("sample.slint", SAMPLE);
    assert_eq!(checked, 2);
    assert_eq!(findings.len(), 1, "{findings:?}");
    assert!(findings[0].contains("`checkbox`"));
}

#[test]
fn spelling_check_reads_strings_not_interpolations() {
    let findings = spelling_findings("sample.slint", SAMPLE);
    assert_eq!(findings.len(), 1, "{findings:?}");
    assert!(findings[0].contains("`text`"));
}
