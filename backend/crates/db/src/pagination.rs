//! Keyset pagination: stable under concurrent inserts and O(log n) per page, unlike OFFSET.
//! The cursor is an opaque base64 encoding of `(created_at, id)` of the last row returned.

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use time::OffsetDateTime;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cursor {
    pub created_at: OffsetDateTime,
    pub id: Uuid,
}

impl Cursor {
    pub fn encode(&self) -> String {
        let nanos = self.created_at.unix_timestamp_nanos();
        URL_SAFE_NO_PAD.encode(format!("{nanos}|{}", self.id))
    }

    /// Invalid cursors are rejected (400) rather than treated as "page one", so client bugs
    /// surface instead of silently re-reading data.
    pub fn decode(s: &str) -> Option<Self> {
        let raw = String::from_utf8(URL_SAFE_NO_PAD.decode(s).ok()?).ok()?;
        let (n, id) = raw.split_once('|')?;
        let created_at = OffsetDateTime::from_unix_timestamp_nanos(n.parse().ok()?).ok()?;
        Some(Self { created_at, id: id.parse().ok()? })
    }
}

/// A page of results plus the cursor for the next page (None when exhausted).
#[derive(Debug, Clone, serde::Serialize, ts_rs::TS)]
#[ts(export)]
pub struct Page<T> {
    pub items: Vec<T>,
    pub next_cursor: Option<String>,
}

/// Clamp client-provided limits.
pub fn clamp_limit(limit: Option<i64>, default: i64, max: i64) -> i64 {
    limit.unwrap_or(default).clamp(1, max)
}

/// Build a page from `limit + 1` fetched rows.
pub fn page_from<T>(mut rows: Vec<T>, limit: i64, key: impl Fn(&T) -> Cursor) -> Page<T> {
    let has_more = rows.len() as i64 > limit;
    rows.truncate(usize::try_from(limit).unwrap_or(0));
    let next_cursor = if has_more { rows.last().map(|r| key(r).encode()) } else { None };
    Page { items: rows, next_cursor }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cursor_round_trip_and_rejects_garbage() {
        let c = Cursor { created_at: OffsetDateTime::now_utc(), id: Uuid::now_v7() };
        assert_eq!(Cursor::decode(&c.encode()), Some(c));
        assert_eq!(Cursor::decode("not-a-cursor"), None);
        assert_eq!(Cursor::decode(""), None);
    }

    #[test]
    fn page_detects_more() {
        let now = OffsetDateTime::now_utc();
        let rows: Vec<(OffsetDateTime, Uuid)> = (0..3).map(|_| (now, Uuid::now_v7())).collect();
        let p = page_from(rows.clone(), 2, |r| Cursor { created_at: r.0, id: r.1 });
        assert_eq!(p.items.len(), 2);
        assert!(p.next_cursor.is_some());
        let p = page_from(rows, 3, |r| Cursor { created_at: r.0, id: r.1 });
        assert!(p.next_cursor.is_none());
    }
}
