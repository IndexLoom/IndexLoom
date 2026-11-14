//! Domain types shared by `IndexLoom` components.

pub mod deployment;
pub mod platform;

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

/// Torrent or Usenet search result protocol.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ReleaseProtocol {
    #[default]
    Torrent,
    Usenet,
}

/// Optional sanitized metadata retained from Cardigann and Newznab.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ReleaseAttributes {
    pub files: Option<u32>,
    pub grabs: Option<u64>,
    pub poster: Option<String>,
    pub genres: Vec<String>,
    pub languages: Vec<String>,
    pub subtitles: Vec<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub author: Option<String>,
    pub publisher: Option<String>,
    pub group: Option<String>,
    pub uploader: Option<String>,
    pub download_volume_factor: Option<f64>,
    pub upload_volume_factor: Option<f64>,
    pub minimum_ratio: Option<f64>,
    pub minimum_seed_time: Option<u64>,
    pub imdb_id: Option<String>,
    pub tmdb_id: Option<String>,
    pub tvdb_id: Option<String>,
    pub tvmaze_id: Option<String>,
    pub trakt_id: Option<String>,
    pub douban_id: Option<String>,
    pub tvrage_id: Option<String>,
}

/// A normalized result returned by an indexer.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchResult {
    /// Ephemeral identifier for this result in one search response.
    pub id: Uuid,
    /// Configured indexer instance that produced this result.
    pub indexer_instance_id: String,
    /// Catalog definition used by the configured instance.
    pub definition_id: String,
    /// Optional tracker-native release identifier.
    pub source_id: Option<String>,
    /// Optional tracker-native globally unique identifier.
    pub source_guid: Option<String>,
    /// Optional normalized `BitTorrent` info hash.
    pub info_hash: Option<String>,
    /// Human-readable release title.
    pub title: String,
    /// Optional tracker details page.
    pub details_url: Option<String>,
    /// Optional direct `.torrent` or download endpoint.
    pub download_url: Option<String>,
    /// Optional magnet URI.
    pub magnet_url: Option<String>,
    /// Normalized or tracker-native category label.
    pub category: Option<String>,
    /// Optional size in bytes.
    pub size: Option<u64>,
    /// Optional seeder count.
    pub seeders: Option<u32>,
    /// Optional leecher count.
    pub leechers: Option<u32>,
    /// Optional publication time.
    pub published_at: Option<OffsetDateTime>,
    #[serde(default)]
    pub protocol: ReleaseProtocol,
    #[serde(default)]
    pub attributes: ReleaseAttributes,
}

#[cfg(test)]
mod tests {
    use serde_json::Value;
    use uuid::Uuid;

    use super::{ReleaseAttributes, ReleaseProtocol, SearchResult};

    #[test]
    fn search_result_matches_the_frontend_contract_fixture() {
        let result = SearchResult {
            id: Uuid::parse_str("019d0000-0000-7000-8000-000000000000").unwrap(),
            indexer_instance_id: "rutracker-through-proxy".to_owned(),
            definition_id: "rutracker".to_owned(),
            source_id: Some("1234567".to_owned()),
            source_guid: None,
            info_hash: Some("0123456789abcdef0123456789abcdef01234567".to_owned()),
            title: "Example release".to_owned(),
            details_url: Some("https://tracker.example/torrent/1234567".to_owned()),
            download_url: Some("https://tracker.example/download/1234567.torrent".to_owned()),
            magnet_url: None,
            category: None,
            size: Some(1_234_567),
            seeders: Some(42),
            leechers: Some(3),
            published_at: None,
            protocol: ReleaseProtocol::Torrent,
            attributes: ReleaseAttributes::default(),
        };
        let rust_json = serde_json::to_value(result).expect("result should serialize");
        let frontend_fixture: Value = serde_json::from_str(include_str!(
            "../../../web/src/lib/search-result.contract.json"
        ))
        .expect("frontend contract fixture should parse");
        assert_eq!(rust_json, frontend_fixture);
    }
}
