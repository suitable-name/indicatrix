//! The in-app help: the user manual in a window of its own, a glossary dialog, and the
//! topic ids a panel's "?" button opens.
//!
//! Help > User Manual used to hand `README.md` to the operating system, which often meant a
//! text editor, found nothing in an installed build and could not point at a section. Now
//! the manual is compiled into the program ([`manual`]) and shown by the help window
//! ([`open_topic`]), which has a chapter list, search, Back and Forward and scrolls to any
//! section.
//!
//! # Pointing at the manual from a panel
//!
//! A topic id is `"<chapter file name>#<section anchor>"` ([`topics`]). The named topics
//! (`TIER_FORM`, `SOLID_VIEW`, ...) are in [`topics::TOPICS`] and repeated for Slint in
//! `HelpTopics` (`ui/models/help.slint`). A panel adds
//!
//! ```text
//! HelpButton { topic: HelpTopics.tier-form; }
//! ```
//!
//! (`ui/components/help_button.slint`); Rust code calls [`open_topic`] with the id. A test
//! resolves every named topic against the embedded manual, so renaming a heading the
//! program points at fails the tests.
//!
//! # Pieces
//!
//! - [`markdown`]: the block parser (headings and anchors, lists, tables, code, quotes);
//! - [`manual`]: the embedded chapters and the chapter list;
//! - [`topics`]: topic ids, the registry and link resolution;
//! - [`context`]: F1, "Help for this screen" -- the topic that fits what is on screen;
//! - [`search`]: search over the whole manual;
//! - [`glossary`]: Appendix A as terms, with [`glossary::glossary_lookup`] for tooltips
//!   that want "More in the glossary";
//! - [`history`]: the Back and Forward trail;
//! - the viewer (`viewer.rs`): the Slint wiring.

pub mod context;
pub mod glossary;
pub mod history;
pub mod manual;
pub mod markdown;
pub mod search;
pub mod topics;
mod viewer;

pub use viewer::{close_help_window, open_topic, setup_help_callbacks};
