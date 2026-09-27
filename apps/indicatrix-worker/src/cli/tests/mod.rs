//! Parser tests for `crate::cli`, one file per subcommand family.
//!
//! - [`help`]: `--help` resolution and unknown subcommands;
//! - [`render`]: the one-shot `render` subcommand;
//! - [`serve`]: the coordinator (`serve`) flags;
//! - [`cert`]: the `cert` sub-subcommands;
//! - [`join`]: the `join` subcommand.

mod cert;
mod help;
mod join;
mod render;
mod serve;
