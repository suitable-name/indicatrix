//! The new-version notice: before a command runs, `main.rs` asks GitHub whether a newer
//! Indicatrix release exists and, only then, prints one `info:` line to standard error (never to
//! standard output, which stays deterministic). The check is skipped with
//! [`NO_UPDATE_CHECK_FLAG`] (removed from the arguments before parsing) or when
//! [`NO_UPDATE_CHECK_ENV`] is set; up to date or any failure prints nothing. The rules live in
//! `indicatrix-update` and are tested there; [`run`](crate::run) and the tests never make a
//! network call.

pub use indicatrix_update::{NO_UPDATE_CHECK_ENV, NO_UPDATE_CHECK_FLAG, take_update_check};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_root_help_names_the_flag_and_the_variable() {
        let page = crate::help::text(crate::args::Topic::Root);
        assert!(page.contains(NO_UPDATE_CHECK_FLAG), "{page}");
        assert!(page.contains(NO_UPDATE_CHECK_ENV), "{page}");
    }
}
