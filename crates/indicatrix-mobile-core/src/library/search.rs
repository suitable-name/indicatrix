//! Searching the library: the page of cards for a [`LibraryQuery`] and the values the
//! filter sheet offers.
//!
//! The vault caps one display query at [`QUERY_ROW_CAP`] rows; paging is a slice of
//! that sorted set, and the total is the exact match count. A query that matches more
//! than the cap can page through the first `QUERY_ROW_CAP` rows of its order only;
//! narrowing the filters reaches the rest.

use indicatrix_vault::{
    db::sqlite::{DisplayFilters, SortOrder},
    model::{entry::DiagramListItem, facets::parse_facets_count, filter::RangeFilter},
};

use super::{FilterOptions, LibraryCard, LibraryHandle, LibraryQuery, handle::db_error};
use crate::settings::LibrarySort;

/// Up to this many designs the shape list is trimmed to the shapes with designs in the
/// library (one cheap count per known shape); beyond it the full vocabulary is offered.
const TRIM_SHAPES_UP_TO: usize = 5_000;

/// The vault's order for a library sort setting.
const fn sort_order(sort: LibrarySort) -> SortOrder {
    match sort {
        LibrarySort::CatalogueOrder => SortOrder::CatalogueOrder,
        LibrarySort::Title => SortOrder::Title,
        LibrarySort::Newest => SortOrder::Newest,
        LibrarySort::RecentlyEdited => SortOrder::RecentlyEdited,
    }
}

/// The number of rows one query can page through: the vault's display cap
/// (`SEARCH_RESULT_CAP`, checked by a test).
pub const QUERY_ROW_CAP: usize = 1_000;

impl LibraryHandle {
    /// One page of the designs matching `query`: `limit` cards from position `offset` of
    /// the sorted result, and the total number of matches.
    ///
    /// # Errors
    /// A sentence when the database cannot be read.
    pub fn search(
        &self,
        query: &LibraryQuery,
        offset: usize,
        limit: usize,
    ) -> Result<(Vec<LibraryCard>, usize), String> {
        let database = self.lock();
        let tag_filter = match query.tag.as_deref().map(str::trim) {
            Some(tag) if !tag.is_empty() => {
                match database
                    .tag_id_by_name(tag)
                    .map_err(db_error("The tags could not be read"))?
                {
                    Some(id) => Some(id),
                    // A tag nobody has: nothing matches.
                    None => return Ok((Vec::new(), 0)),
                }
            }
            _ => None,
        };
        let range = RangeFilter {
            ri_min: query.ri_min,
            ri_max: query.ri_max,
            include_ignored: query.show_ignored,
            ..RangeFilter::default()
        };
        let filters = DisplayFilters {
            order: sort_order(query.sort),
            local_only: query.mine_only,
            tag_filter,
            id_filter: None,
        };
        let shape = query.shape.as_deref().unwrap_or_default();
        let gear = query.gear.map(|gear| gear.to_string()).unwrap_or_default();
        let (found, total) = database
            .search_diagrams_display_with_count(&query.text, shape, &gear, &range, filters)
            .map_err(db_error("The library could not be searched"))?;
        let page: Vec<DiagramListItem> = found.items.into_iter().skip(offset).take(limit).collect();
        let ids: Vec<i64> = page.iter().map(|item| item.id).collect();
        let mut tags = database
            .tags_for_entries(&ids)
            .map_err(db_error("The tags could not be read"))?;
        let cards = page
            .into_iter()
            .map(|item| {
                let updated_unix = database
                    .entry_updated_at(item.id)
                    .ok()
                    .flatten()
                    .unwrap_or(0);
                let entry_tags = tags.remove(&item.id).unwrap_or_default();
                card_of(item, entry_tags, updated_unix)
            })
            .collect();
        drop(database);
        Ok((cards, total))
    }

    /// The values the filter sheet offers: the shapes and gears the library has designs
    /// of and every tag.
    ///
    /// # Errors
    /// A sentence when the database cannot be read.
    pub fn filter_options(&self) -> Result<FilterOptions, String> {
        let database = self.lock();
        let total = database
            .get_total_count()
            .map_err(db_error("The library could not be counted"))?;
        let mut shapes = database
            .get_unique_shapes()
            .map_err(db_error("The shapes could not be read"))?;
        if total <= TRIM_SHAPES_UP_TO {
            let everything = RangeFilter {
                include_ignored: true,
                ..RangeFilter::default()
            };
            shapes.retain(|shape| {
                database
                    .count_matching_diagrams("", shape, "", &everything, DisplayFilters::default())
                    .map_or(true, |count| count > 0)
            });
        }
        let mut gears: Vec<u32> = database
            .get_unique_gears()
            .map_err(db_error("The gears could not be read"))?
            .iter()
            .filter_map(|gear| gear.trim().parse::<u32>().ok())
            .collect();
        gears.sort_unstable();
        gears.dedup();
        let tags = database
            .list_tags()
            .map_err(db_error("The tags could not be read"))?
            .into_iter()
            .map(|tag| tag.name)
            .collect();
        drop(database);
        Ok(FilterOptions {
            shapes,
            gears,
            tags,
        })
    }
}

/// The card for one search row.
fn card_of(item: DiagramListItem, tags: Vec<String>, updated_unix: i64) -> LibraryCard {
    let facets = item.facets_count.as_deref();
    let (main, girdle) = parse_facets_count(facets);
    let facet_total = main.unwrap_or(0).saturating_add(girdle.unwrap_or(0));
    let designer = item
        .designer_info
        .as_deref()
        .and_then(|line| line.split(';').next())
        .map(str::trim)
        .unwrap_or_default()
        .to_owned();
    LibraryCard {
        id: item.id,
        title: item.title,
        designer,
        shape: item.shape.unwrap_or_default(),
        gear: item
            .index_gear
            .as_deref()
            .and_then(|gear| gear.trim().parse::<u32>().ok())
            .unwrap_or(0),
        facets: u32::try_from(facet_total).unwrap_or(0),
        ri: item
            .refractive_index
            .as_deref()
            .and_then(|ri| ri.trim().parse::<f64>().ok()),
        tags,
        mine: item.url.starts_with("local://"),
        ignored: item.ignored,
        updated_unix,
    }
}
