//! Walks the remote catalogue's own paginated listing to exhaustion.

use crate::bridge::library::mirror::options::{LibraryTransport, MirrorOutcome};
use indicatrix_net::library::{LibraryRequest, LibraryResponse, RangeFilterWire};

/// Walks [`LibraryRequest::SearchPage`] to exhaustion, concatenating every page's
/// [`DesignSummary`](indicatrix_net::library::DesignSummary) list into one `Vec` -- the
/// WHOLE remote catalogue matching the (currently always empty/unfiltered) query, not
/// just its first page. See the mirror module's own doc comment's "Pagination" section
/// for the keyset-cursor scheme this implements (`cursor: None`, then `cursor:
/// Some(next_cursor)` from each reply, until a reply's `next_cursor` is `None`) and
/// why it never needs to skip or re-see a row even if designs are added to the remote
/// catalogue while this runs.
///
/// Returns `Err(MirrorOutcome::Failed(..))` -- ready to propagate straight out of
/// `super::pass::run_mirror_sync` -- on the first request that fails outright or
/// replies with anything other than [`LibraryResponse::SearchResultsPage`]; nothing is
/// written locally by this function (it only ever reads from `transport`), so a
/// failure here leaves the local database exactly as it was, matching this module's
/// existing "nothing written until enumeration succeeds" behavior.
pub(super) fn enumerate_remote_catalogue(
    transport: &impl LibraryTransport,
) -> Result<Vec<indicatrix_net::library::DesignSummary>, MirrorOutcome> {
    let mut summaries = Vec::new();
    let mut cursor = None;
    loop {
        let request = LibraryRequest::SearchPage {
            query: String::new(),
            shape_filter: "All".to_string(),
            gear_filter: "All".to_string(),
            range: RangeFilterWire::default(),
            cursor,
        };
        match transport.request(&request) {
            Ok(LibraryResponse::SearchResultsPage {
                results,
                next_cursor,
                // This exhaustive, unfiltered walk never sets `range.performance`, so
                // there is nothing this count could ever report here -- see
                // `apps/indicatrix-worker::serve::library::search_page`'s own doc comment
                // on why `SearchResultsPage` always sends `0` for exactly that reason.
                excluded_for_missing_curves: _,
            }) => {
                summaries.extend(results);
                match next_cursor {
                    Some(c) => cursor = Some(c),
                    None => return Ok(summaries),
                }
            }
            Ok(other) => {
                return Err(MirrorOutcome::Failed(format!(
                    "remote worker replied to SearchPage with an unexpected message: {other:?}"
                )));
            }
            Err(e) => {
                return Err(MirrorOutcome::Failed(format!(
                    "could not list the remote library: {e}"
                )));
            }
        }
    }
}
