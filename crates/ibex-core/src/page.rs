//! Pagination, shared by every listing in the crate.
//!
//! Anything that can be large is paginated; nothing returns an unbounded list.
//! See AGENTS.md ("Paginate anything that can be large").

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// A request for one window of results.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Page {
    pub offset: u64,
    pub limit: u32,
}

impl Page {
    /// How many items a listing returns when the caller doesn't say.
    pub const DEFAULT_LIMIT: u32 = 200;

    /// The largest window any single call will return, so a caller cannot ask
    /// for a whole database in one go.
    pub const MAX_LIMIT: u32 = 2_000;

    pub fn new(offset: u64, limit: u32) -> Self {
        Self {
            offset,
            limit: limit.clamp(1, Self::MAX_LIMIT),
        }
    }

    pub fn first(limit: u32) -> Self {
        Self::new(0, limit)
    }

    /// The limit as SQLite wants it.
    pub(crate) fn sql_limit(&self) -> i64 {
        i64::from(self.limit)
    }

    pub(crate) fn sql_offset(&self) -> i64 {
        i64::try_from(self.offset).unwrap_or(i64::MAX)
    }
}

impl Default for Page {
    fn default() -> Self {
        Self::first(Self::DEFAULT_LIMIT)
    }
}

/// One window of results, plus enough information to ask for the next.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Paged<T> {
    pub items: Vec<T>,
    /// Total number of items available, when it is cheap to know.
    pub total: Option<u64>,
    /// Offset to request next, or `None` at the end of the listing.
    pub next_offset: Option<u64>,
}

impl<T> Paged<T> {
    /// Build a page from the items for `page`, given the total count.
    pub fn new(items: Vec<T>, page: Page, total: Option<u64>) -> Self {
        let consumed = page.offset.saturating_add(items.len() as u64);
        let more = match total {
            Some(total) => consumed < total,
            // Without a total, a full window suggests there may be more.
            None => items.len() as u32 == page.limit,
        };
        Self {
            items,
            total,
            next_offset: more.then_some(consumed),
        }
    }

    pub fn empty() -> Self {
        Self {
            items: Vec::new(),
            total: Some(0),
            next_offset: None,
        }
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limits_are_clamped() {
        assert_eq!(Page::new(0, 0).limit, 1);
        assert_eq!(Page::new(0, 10_000).limit, Page::MAX_LIMIT);
    }

    #[test]
    fn next_offset_follows_the_total() {
        let p = Paged::new(vec![1, 2], Page::new(0, 2), Some(5));
        assert_eq!(p.next_offset, Some(2));
        let p = Paged::new(vec![1, 2], Page::new(3, 2), Some(5));
        assert_eq!(p.next_offset, None, "last window ends the listing");
    }

    #[test]
    fn without_a_total_a_full_window_implies_more() {
        let p = Paged::new(vec![1, 2], Page::new(0, 2), None);
        assert_eq!(p.next_offset, Some(2));
        let p = Paged::new(vec![1], Page::new(0, 2), None);
        assert_eq!(p.next_offset, None);
    }
}
