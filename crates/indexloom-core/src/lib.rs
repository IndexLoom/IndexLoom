//! Domain types shared by `IndexLoom` components.

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use uuid::Uuid;

/// A normalized search request independent of any indexer implementation.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchQuery {
    /// Free-form user search text.
    pub term: String,
    /// Optional normalized category identifiers.
    #[serde(default)]
    pub categories: Vec<String>,
    /// Maximum desired number of results.
    pub limit: Option<u32>,
}

/// A normalized result returned by an indexer.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchResult {
    /// Stable result identifier within one search run.
    pub id: Uuid,
    /// Definition identifier that produced this result.
    pub indexer_id: String,
    /// Human-readable release title.
    pub title: String,
    /// Download or details URL.
    pub link: String,
    /// Optional size in bytes.
    pub size: Option<u64>,
    /// Optional seeder count.
    pub seeders: Option<u32>,
    /// Optional publication time.
    pub published_at: Option<OffsetDateTime>,
}
