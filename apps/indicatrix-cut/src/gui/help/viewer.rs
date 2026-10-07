//! The help window and the glossary dialog: the Slint half of the in-app help.
//!
//! The window is created on the first request, hidden (never destroyed) when it is closed
//! so Back and Forward survive, and closed together with the main window. It has its own
//! `HelpModel` (a Slint global exists once per window), which this module fills: the
//! chapter list, the open chapter's blocks, the search results.
//!
//! The main window's `HelpModel` only carries the requests that reach here
//! ([`setup_help_callbacks`]) and the glossary dialog's list.

use super::{
    glossary::{self, GlossaryEntry},
    history::Trail,
    manual::{self, NavRow, nav_rows},
    markdown::{Block, BlockKind, plain_text},
    search::{self, Hit},
    topics::{self, Link},
};
use crate::{
    GlossaryRow, HelpBlock, HelpModel, HelpNavItem, HelpSearchHit, HelpWindow, MainWindow, Theme,
    gui::{external_links, show_toast, tutorial_events::raise},
};
use indicatrix_editor::guide::viewing_events as events;
use slint::{
    CloseRequestResponse, ComponentHandle, ModelRc, SharedString, StyledText, Timer, TimerMode,
    VecModel, Weak, winit_030::WinitWindowAccessor,
};
use std::{cell::RefCell, rc::Rc, time::Duration};

/// How often the open window checks that the main window is still there, and follows
/// its high-contrast setting.
const WATCH_INTERVAL: Duration = Duration::from_millis(300);

/// The most search results the window lists.
const MAX_HITS: usize = 50;

const TOPIC_MISSING: &str = "That page of the manual was not found. Showing the contents.";
const SECTION_MISSING: &str =
    "That part of the manual was not found. Showing the top of the chapter.";
const LINK_UNSUPPORTED: &str = "That link cannot be opened from the help window.";
const FILES_MISSING: &str = "The manual files are not installed next to the program. \
     This window shows the copy built into the program.";

/// A place in the manual: a chapter and a section of it (`0` is the chapter's top).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Place {
    chapter: usize,
    section: usize,
}

#[derive(Default)]
struct State {
    /// Where Back and Forward lead.
    trail: Trail<Place>,
    /// The chapter whose blocks the window holds, so moving inside a chapter does not
    /// rebuild the page.
    shown_chapter: Option<usize>,
}

/// Everything the help window's callbacks reach on the UI thread.
struct Host {
    window: HelpWindow,
    main: Weak<MainWindow>,
    /// While the window is open: ends it when the main window goes, and keeps its colours
    /// in step with the main window's.
    watch: Timer,
    state: RefCell<State>,
}

thread_local! {
    /// The one help window of the session, once it was opened.
    static HELP: RefCell<Option<Rc<Host>>> = const { RefCell::new(None) };
}

/// Runs `f` with the help host; nothing happens when the window does not exist (never
/// opened, or closed with the main window). The host is cloned out first, so `f` may call
/// this again.
fn on_host(f: impl FnOnce(&Host)) {
    if let Some(host) = HELP.with(|cell| cell.borrow().clone()) {
        f(&host);
    }
}

fn to_i32(value: usize) -> i32 {
    i32::try_from(value).unwrap_or(i32::MAX)
}

// --- Converting parsed blocks for Slint ---------------------------------------------

/// Inline Markdown as Slint styled text. Text that Slint's Markdown subset refuses (raw
/// HTML tags, a leading `>`) is shown as plain words instead of being lost.
fn styled_text(markdown: &str) -> StyledText {
    StyledText::from_markdown(markdown)
        .unwrap_or_else(|_| StyledText::from_plain_text(&plain_text(markdown)))
}

fn slint_block(block: &Block) -> HelpBlock {
    let rich = matches!(
        block.kind,
        BlockKind::Paragraph | BlockKind::ListItem | BlockKind::Quote
    );
    let plain = matches!(
        block.kind,
        BlockKind::Heading | BlockKind::Code | BlockKind::Figure
    );
    HelpBlock {
        kind: block.kind.code(),
        level: i32::from(block.level),
        indent: i32::from(block.indent),
        marker: block.marker.as_str().into(),
        text: if rich {
            styled_text(&block.text)
        } else {
            StyledText::default()
        },
        plain: if plain {
            block.text.as_str().into()
        } else {
            SharedString::new()
        },
        cells: ModelRc::new(VecModel::from(
            block
                .cells
                .iter()
                .map(|cell| styled_text(cell))
                .collect::<Vec<_>>(),
        )),
        weights: ModelRc::new(VecModel::from(block.weights.clone())),
        header: block.header,
        section: to_i32(block.section),
    }
}

fn blocks_model(blocks: &[Block]) -> ModelRc<HelpBlock> {
    ModelRc::new(VecModel::from(
        blocks.iter().map(slint_block).collect::<Vec<_>>(),
    ))
}

fn nav_model(rows: &[NavRow]) -> ModelRc<HelpNavItem> {
    ModelRc::new(VecModel::from(
        rows.iter()
            .map(|row| HelpNavItem {
                label: row.label.as_str().into(),
                topic: row.topic.as_str().into(),
                level: i32::from(row.level),
                open: row.open,
                current: row.current,
            })
            .collect::<Vec<_>>(),
    ))
}

fn hits_model(hits: &[Hit]) -> ModelRc<HelpSearchHit> {
    let chapters = manual::chapters();
    let rows: Vec<HelpSearchHit> = hits
        .iter()
        .filter_map(|hit| {
            let chapter = chapters.get(hit.chapter)?;
            let section = chapter.parsed.sections.get(hit.section)?;
            let on_title = hit.section == 0 || section.slug.is_empty();
            Some(HelpSearchHit {
                topic: if on_title {
                    chapter.stem.into()
                } else {
                    format!("{}#{}", chapter.stem, section.slug).into()
                },
                chapter: chapter.label.as_str().into(),
                section: if on_title {
                    SharedString::new()
                } else {
                    section.title.as_str().into()
                },
                snippet: hit.snippet.as_str().into(),
            })
        })
        .collect();
    ModelRc::new(VecModel::from(rows))
}

fn glossary_row(entry: &GlossaryEntry) -> GlossaryRow {
    GlossaryRow {
        term: entry.term.as_str().into(),
        text: styled_text(&entry.text),
    }
}

fn glossary_rows(query: &str) -> ModelRc<GlossaryRow> {
    let rows: Vec<GlossaryRow> = glossary::filter(glossary::entries(), query)
        .into_iter()
        .map(glossary_row)
        .collect();
    ModelRc::new(VecModel::from(rows))
}

// --- The window ----------------------------------------------------------------------

fn set_status(host: &Host, text: &str) {
    host.window.global::<HelpModel>().set_status(text.into());
}

/// Shows `place`: loads its chapter if it is not the one on screen, marks it in the
/// chapter list and asks the page to scroll to its section. `record` adds it to the
/// Back trail (not when Back and Forward themselves moved).
fn show_place(host: &Host, place: Place, record: bool) {
    let chapters = manual::chapters();
    let Some(chapter) = chapters.get(place.chapter) else {
        return;
    };
    let (reload, can_back, can_forward) = {
        let mut state = host.state.borrow_mut();
        if record {
            state.trail.visit(place);
        }
        let reload = state.shown_chapter != Some(place.chapter);
        state.shown_chapter = Some(place.chapter);
        (
            reload,
            state.trail.can_go_back(),
            state.trail.can_go_forward(),
        )
    };
    let model = host.window.global::<HelpModel>();
    if reload {
        model.set_blocks(blocks_model(&chapter.parsed.blocks));
        model.set_chapter_title(chapter.label.as_str().into());
    }
    model.set_nav(nav_model(&nav_rows(chapters, place.chapter, place.section)));
    model.set_scroll_section(to_i32(place.section));
    model.set_scroll_pending(true);
    model.set_can_back(can_back);
    model.set_can_forward(can_forward);
    model.set_searching(false);
    model.set_status(SharedString::new());
}

/// Opens the topic `id`. An id that names no chapter shows the contents, and one that
/// names no section the top of its chapter, each with a line saying so.
fn go_to_topic(host: &Host, id: &str) {
    let (place, note) = topics::resolve(id).map_or(
        (
            Place {
                chapter: 0,
                section: 0,
            },
            TOPIC_MISSING,
        ),
        |target| {
            (
                Place {
                    chapter: target.chapter,
                    section: target.section,
                },
                if target.exact { "" } else { SECTION_MISSING },
            )
        },
    );
    show_place(host, place, true);
    set_status(host, note);
}

/// Back (`forward == false`) or Forward.
fn step(host: &Host, forward: bool) {
    let place = {
        let mut state = host.state.borrow_mut();
        if forward {
            state.trail.go_forward()
        } else {
            state.trail.go_back()
        }
    };
    if let Some(place) = place {
        show_place(host, place, false);
    }
}

/// A link in the page: another part of the manual, or a web address for the browser.
fn follow_link(host: &Host, link: &str) {
    let current = host
        .state
        .borrow()
        .shown_chapter
        .and_then(|index| manual::chapters().get(index))
        .map_or(manual::CONTENTS_STEM, |chapter| chapter.stem);
    match topics::classify_link(link, current) {
        Link::Topic(id) => go_to_topic(host, &id),
        Link::External(url) => {
            let problem = external_links::open_external_url(&url).err();
            set_status(host, problem.as_deref().unwrap_or_default());
        }
        Link::Unsupported => set_status(host, LINK_UNSUPPORTED),
    }
}

fn run_search(host: &Host, query: &str) {
    let model = host.window.global::<HelpModel>();
    if query.trim().chars().count() < search::MIN_QUERY_CHARS {
        model.set_searching(false);
        model.set_hits(ModelRc::default());
        return;
    }
    model.set_hits(hits_model(&search::search(query, MAX_HITS)));
    model.set_searching(true);
}

/// "Open manual folder": the folder with the plain `.md` files, when the program is
/// installed next to them.
fn open_manual_files(host: &Host) {
    let problem = external_links::locate_manual_folder().map_or_else(
        || Some(FILES_MISSING.to_owned()),
        |folder| external_links::open_local_path(&folder).err(),
    );
    set_status(host, problem.as_deref().unwrap_or_default());
}

fn close_window(host: &Host) {
    host.watch.stop();
    let _ = host.window.hide();
}

/// The window follows the main window's high-contrast setting.
fn sync_theme(main: &MainWindow, window: &HelpWindow) {
    let wanted = main.global::<Theme>().get_high_contrast();
    let theme = window.global::<Theme>();
    if theme.get_high_contrast() != wanted {
        theme.set_high_contrast(wanted);
    }
}

/// One tick of the watch timer: the help window goes when the main window does (it would
/// otherwise keep the program running), and takes over its palette.
fn watch_tick(host: &Host) {
    match host.main.upgrade() {
        Some(main) if main.window().is_visible() => sync_theme(&main, &host.window),
        _ => close_window(host),
    }
}

fn register_callbacks(window: &HelpWindow) {
    let model = window.global::<HelpModel>();
    model.on_open_topic(|id| on_host(|host| go_to_topic(host, id.as_str())));
    model.on_link_clicked(|link| on_host(|host| follow_link(host, link.as_str())));
    model.on_back(|| on_host(|host| step(host, false)));
    model.on_forward(|| on_host(|host| step(host, true)));
    model.on_search(|query| on_host(|host| run_search(host, query.as_str())));
    model.on_open_manual_files(|| on_host(open_manual_files));
    model.on_close_window(|| on_host(close_window));
    // Closing hides the window; the host and its Back trail stay.
    window.window().on_close_requested(|| {
        on_host(|host| host.watch.stop());
        CloseRequestResponse::HideWindow
    });
}

fn create_host(main: &MainWindow) -> Result<Rc<Host>, String> {
    let window =
        HelpWindow::new().map_err(|error| format!("Could not open the help window: {error}"))?;
    register_callbacks(&window);
    let host = Rc::new(Host {
        window,
        main: main.as_weak(),
        watch: Timer::default(),
        state: RefCell::new(State::default()),
    });
    HELP.with(|cell| *cell.borrow_mut() = Some(Rc::clone(&host)));
    Ok(host)
}

/// Opens the help window at the topic `topic` (see `help::topics`): creates it on the
/// first call, otherwise brings the existing one forward.
///
/// # Errors
///
/// A ready-to-toast message when the window cannot be created or shown.
pub fn open_topic(main: &MainWindow, topic: &str) -> Result<(), String> {
    let host = if let Some(host) = HELP.with(|cell| cell.borrow().clone()) {
        host
    } else {
        create_host(main)?
    };
    go_to_topic(&host, topic);
    sync_theme(main, &host.window);
    host.window
        .show()
        .map_err(|error| format!("Could not show the help window: {error}"))?;
    host.window.window().set_minimized(false);
    // The platform window is raised on the first show by itself; this is for a window
    // that is already open behind the main one.
    let _ = host
        .window
        .window()
        .with_winit_window(slint::winit_030::winit::window::Window::focus_window);
    host.watch
        .start(TimerMode::Repeated, WATCH_INTERVAL, || on_host(watch_tick));
    // A tutorial step may wait for the manual to open.
    raise(main, events::HELP_OPENED);
    Ok(())
}

/// Closes the help window for good. Called where the main window hides, so the help
/// window never outlives it.
pub fn close_help_window() {
    let Some(host) = HELP.with(|cell| cell.borrow_mut().take()) else {
        return;
    };
    close_window(&host);
}

/// Opens the glossary dialog on the main window, filtered to `term` (every term when it
/// is empty). A term the glossary knows under another name (`RI`, `tiers`) is replaced by
/// the glossary's own spelling, so the dialog lists that entry first.
fn open_glossary(ui: &MainWindow, term: &str) {
    let query = glossary::glossary_lookup(term)
        .map_or_else(|| term.trim().to_owned(), |entry| entry.term.clone());
    let model = ui.global::<HelpModel>();
    model.set_glossary_rows(glossary_rows(&query));
    model.set_glossary_query(query.into());
    model.set_glossary_open(true);
    // A tutorial step may wait for the glossary to open.
    raise(ui, events::GLOSSARY_OPENED);
}

/// Wires the main window's `HelpModel`: opening a topic in the help window (from the Help
/// menu, a "?" button or the planner), and the glossary dialog.
pub fn setup_help_callbacks(ui: &MainWindow) {
    let model = ui.global::<HelpModel>();
    model.set_glossary_total(to_i32(glossary::entries().len()));

    let weak = ui.as_weak();
    model.on_open_topic(move |topic| {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        if let Err(message) = open_topic(&ui, topic.as_str()) {
            show_toast(&ui, &message, "error");
        }
    });

    let weak = ui.as_weak();
    model.on_open_glossary(move |term| {
        if let Some(ui) = weak.upgrade() {
            open_glossary(&ui, term.as_str());
        }
    });

    let weak = ui.as_weak();
    model.on_glossary_search(move |query| {
        if let Some(ui) = weak.upgrade() {
            ui.global::<HelpModel>()
                .set_glossary_rows(glossary_rows(query.as_str()));
        }
    });

    super::context::setup_context_help(ui);
}

#[cfg(test)]
mod tests {
    use super::*;
    use slint::Model;

    fn block(kind: BlockKind, text: &str) -> Block {
        Block {
            kind,
            text: text.to_owned(),
            ..Block::default()
        }
    }

    #[test]
    fn a_paragraph_keeps_its_formatting_as_styled_text() {
        let item = slint_block(&block(BlockKind::Paragraph, "a **bold** word"));
        assert_eq!(item.kind, 0);
        assert_eq!(
            item.text,
            StyledText::from_markdown("a **bold** word").expect("valid markdown")
        );
        assert_eq!(item.plain, "");
    }

    #[test]
    fn a_heading_and_a_code_block_carry_plain_text() {
        let heading = slint_block(&Block {
            level: 2,
            ..block(BlockKind::Heading, "The tier form")
        });
        assert_eq!((heading.kind, heading.level), (1, 2));
        assert_eq!(heading.plain, "The tier form");
        assert_eq!(heading.text, StyledText::default());
        let code = slint_block(&block(BlockKind::Code, "a *b*\n  c"));
        assert_eq!(code.plain, "a *b*\n  c");
    }

    #[test]
    fn a_table_row_carries_its_cells_and_column_weights() {
        let item = slint_block(&Block {
            cells: vec!["Key".to_owned(), "**Action**".to_owned()],
            weights: vec![6.0, 40.0],
            header: true,
            ..block(BlockKind::TableRow, "")
        });
        assert_eq!(item.kind, 5);
        assert_eq!(item.cells.row_count(), 2);
        assert_eq!(item.weights.row_data(1), Some(40.0));
        assert!(item.header);
    }

    #[test]
    fn markdown_the_viewer_cannot_render_falls_back_to_plain_words() {
        // A leading ">" makes a block quote, which Slint's styled text refuses.
        assert!(StyledText::from_markdown("> quoted").is_err());
        assert_eq!(
            styled_text("> quoted **words**"),
            StyledText::from_plain_text("> quoted words")
        );
    }

    #[test]
    fn nav_rows_convert_one_to_one() {
        let rows = vec![NavRow {
            label: "Contents".to_owned(),
            topic: "README".to_owned(),
            level: 0,
            open: true,
            current: true,
        }];
        let model = nav_model(&rows);
        let item = model.row_data(0).expect("one row");
        assert_eq!(
            (item.label.as_str(), item.topic.as_str()),
            ("Contents", "README")
        );
        assert!(item.open && item.current);
    }

    #[test]
    fn search_hits_name_their_chapter_and_section() {
        let hits = search::search("tier form", 5);
        let model = hits_model(&hits);
        assert_eq!(model.row_count(), hits.len());
        let first = model.row_data(0).expect("a hit");
        assert!(first.topic.contains('#'), "{}", first.topic);
        assert!(!first.chapter.is_empty() && !first.section.is_empty());
        // A hit on a chapter's own title opens the chapter without an anchor.
        let top = hits_model(&[Hit {
            chapter: 0,
            section: 0,
            score: 1,
            snippet: String::new(),
        }]);
        let row = top.row_data(0).expect("a row");
        assert_eq!((row.topic.as_str(), row.section.as_str()), ("README", ""));
    }

    #[test]
    fn glossary_rows_follow_the_filter() {
        let all = glossary_rows("");
        assert_eq!(all.row_count(), glossary::entries().len());
        let tier = glossary_rows("tier");
        assert_eq!(
            tier.row_data(0).map(|row| row.term.to_string()),
            Some("Tier".to_owned())
        );
        assert_eq!(glossary_rows("zzzzqqqq").row_count(), 0);
    }

    #[test]
    fn the_integer_conversion_never_wraps() {
        assert_eq!(to_i32(7), 7);
        assert_eq!(to_i32(usize::MAX), i32::MAX);
    }
}
