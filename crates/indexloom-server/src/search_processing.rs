//! Deterministic global result view. Grouped per-indexer rows are retained by the API.
use indexloom_core::{ReleaseProtocol, SearchResult};
use serde::{Deserialize, Serialize};
use std::{cmp::Ordering, collections::HashMap};
use time::OffsetDateTime;

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum SortField {
    #[default]
    PublishedAt,
    Size,
    Seeders,
    Leechers,
    Grabs,
    Files,
    Indexer,
    Title,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ResultOptions {
    pub deduplicate: bool,
    pub sort_by: SortField,
    pub descending: bool,
    pub min_size: Option<u64>,
    pub max_size: Option<u64>,
    pub min_seeders: Option<u32>,
    pub max_age_days: Option<u32>,
    pub protocol: Option<ReleaseProtocol>,
    pub categories: Vec<String>,
    pub include_indexers: Vec<String>,
    pub exclude_indexers: Vec<String>,
    pub title_contains: Option<String>,
    pub indexer_priorities: HashMap<String, i32>,
}
impl Default for ResultOptions {
    fn default() -> Self {
        Self {
            deduplicate: true,
            sort_by: SortField::PublishedAt,
            descending: true,
            min_size: None,
            max_size: None,
            min_seeders: None,
            max_age_days: None,
            protocol: None,
            categories: Vec::new(),
            include_indexers: Vec::new(),
            exclude_indexers: Vec::new(),
            title_contains: None,
            indexer_priorities: HashMap::new(),
        }
    }
}
impl ResultOptions {
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.min_size.zip(self.max_size).is_some_and(|(a, b)| a > b)
            || self.max_age_days.is_some_and(|days| days > 36500)
            || self.categories.len() > 64
            || self.include_indexers.len() > 64
            || self.exclude_indexers.len() > 64
            || self.indexer_priorities.len() > 256
            || self
                .title_contains
                .as_ref()
                .is_some_and(|v| v.len() > 128 || v.chars().any(char::is_control))
            || self
                .categories
                .iter()
                .chain(&self.include_indexers)
                .chain(&self.exclude_indexers)
                .any(|v| v.len() > 128 || v.chars().any(char::is_control))
            || self
                .indexer_priorities
                .iter()
                .any(|(id, rank)| id.len() > 128 || !(0..=100).contains(rank))
        {
            return Err("invalid result filter or sorting options");
        }
        Ok(())
    }

    fn matches(&self, release: &SearchResult, now: OffsetDateTime) -> bool {
        if self.protocol.is_some_and(|p| p != release.protocol)
            || self
                .min_size
                .is_some_and(|n| release.size.is_none_or(|v| v < n))
            || self
                .max_size
                .is_some_and(|n| release.size.is_none_or(|v| v > n))
            || self
                .min_seeders
                .is_some_and(|n| release.seeders.is_none_or(|v| v < n))
            || !self.include_indexers.is_empty()
                && !self.include_indexers.contains(&release.indexer_instance_id)
            || self.exclude_indexers.contains(&release.indexer_instance_id)
            || !self.categories.is_empty()
                && !release
                    .category
                    .as_ref()
                    .is_some_and(|v| self.categories.contains(v))
            || self.title_contains.as_ref().is_some_and(|needle| {
                !release
                    .title
                    .to_lowercase()
                    .contains(&needle.to_lowercase())
            })
        {
            return false;
        }
        if let Some(days) = self.max_age_days {
            let Some(published) = release.published_at else {
                return false;
            };
            if (now - published).whole_days() > i64::from(days) {
                return false;
            }
        }
        true
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProcessedResults {
    pub unique_results: Vec<SearchResult>,
    pub duplicate_count: usize,
    pub filtered_count: usize,
}

fn identity(row: &SearchResult) -> Option<String> {
    if row.protocol == ReleaseProtocol::Torrent
        && let Some(hash) = row.info_hash.as_ref()
        && hash.len() == 40
        && hash.bytes().all(|b| b.is_ascii_hexdigit())
    {
        return Some(format!("btih:{}", hash.to_ascii_lowercase()));
    }
    if let Some(guid) = row
        .source_guid
        .as_ref()
        .filter(|g| g.len() <= 2048 && !g.is_empty())
    {
        if let Ok(url) = url::Url::parse(guid) {
            if matches!(url.scheme(), "http" | "https") {
                return Some(format!(
                    "url:{}://{}{}",
                    url.scheme(),
                    url.host_str()?,
                    url.path()
                ));
            }
        } else if guid.starts_with("urn:") {
            return Some(format!("urn:{guid}"));
        }
        return Some(format!("guid:{}:{guid}", row.definition_id));
    }
    let title = row
        .title
        .chars()
        .filter(|v| v.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .take(256)
        .collect::<String>();
    if !title.is_empty()
        && let Some(size) = row.size.filter(|n| *n > 0)
    {
        return Some(format!(
            "{}:title:{title}:size:{size}",
            match row.protocol {
                ReleaseProtocol::Torrent => "torrent",
                ReleaseProtocol::Usenet => "usenet",
            }
        ));
    }
    None
}

fn prefer(a: &SearchResult, b: &SearchResult, options: &ResultOptions) -> bool {
    let priority = |row: &SearchResult| {
        options
            .indexer_priorities
            .get(&row.indexer_instance_id)
            .copied()
            .unwrap_or(50)
    };
    priority(b)
        .cmp(&priority(a))
        .then_with(|| b.seeders.unwrap_or(0).cmp(&a.seeders.unwrap_or(0)))
        .then_with(|| {
            b.attributes
                .grabs
                .unwrap_or(0)
                .cmp(&a.attributes.grabs.unwrap_or(0))
        })
        .then_with(|| b.published_at.cmp(&a.published_at))
        .then_with(|| a.indexer_instance_id.cmp(&b.indexer_instance_id))
        == Ordering::Greater
}

fn compare(a: &SearchResult, b: &SearchResult, field: SortField) -> Ordering {
    match field {
        SortField::PublishedAt => a.published_at.cmp(&b.published_at),
        SortField::Size => a.size.cmp(&b.size),
        SortField::Seeders => a.seeders.cmp(&b.seeders),
        SortField::Leechers => a.leechers.cmp(&b.leechers),
        SortField::Grabs => a.attributes.grabs.cmp(&b.attributes.grabs),
        SortField::Files => a.attributes.files.cmp(&b.attributes.files),
        SortField::Indexer => a.indexer_instance_id.cmp(&b.indexer_instance_id),
        SortField::Title => a.title.to_lowercase().cmp(&b.title.to_lowercase()),
    }
}
pub fn process(
    rows: impl IntoIterator<Item = SearchResult>,
    opts: &ResultOptions,
) -> ProcessedResults {
    let now = OffsetDateTime::now_utc();
    let mut filtered = 0;
    let mut duplicates = 0;
    let mut identities = HashMap::<String, usize>::new();
    let mut items = Vec::<SearchResult>::new();
    for release in rows {
        if !opts.matches(&release, now) {
            filtered += 1;
            continue;
        }
        if opts.deduplicate
            && let Some(key) = identity(&release)
        {
            if let Some(&prior) = identities.get(&key) {
                duplicates += 1;
                if prefer(&items[prior], &release, opts) {
                    items[prior] = release;
                }
                continue;
            }
            identities.insert(key, items.len());
        }
        items.push(release);
    }
    items.sort_by(|a, b| {
        let order = compare(a, b, opts.sort_by);
        let order = if opts.descending {
            order.reverse()
        } else {
            order
        };
        order
            .then_with(|| a.indexer_instance_id.cmp(&b.indexer_instance_id))
            .then_with(|| a.title.cmp(&b.title))
    });
    ProcessedResults {
        unique_results: items,
        duplicate_count: duplicates,
        filtered_count: filtered,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use indexloom_core::ReleaseAttributes;
    use uuid::Uuid;
    fn row(title: &str, site: &str, hash: Option<&str>, bytes: u64, seeds: u32) -> SearchResult {
        SearchResult {
            id: Uuid::nil(),
            indexer_instance_id: site.into(),
            definition_id: site.into(),
            source_id: None,
            source_guid: None,
            info_hash: hash.map(ToOwned::to_owned),
            title: title.into(),
            details_url: None,
            download_url: None,
            magnet_url: None,
            category: Some("2000".into()),
            size: Some(bytes),
            seeders: Some(seeds),
            leechers: Some(0),
            published_at: None,
            protocol: ReleaseProtocol::Torrent,
            attributes: ReleaseAttributes::default(),
        }
    }
    #[test]
    fn prefer_configured_priority_and_never_merge_different_hashes() {
        let mut o = ResultOptions::default();
        o.indexer_priorities.insert("second".into(), 100);
        let h = "0123456789abcdef0123456789abcdef01234567";
        let output = process(
            [
                row("ubuntu", "first", Some(h), 1, 99),
                row("Ubuntu", "second", Some(h), 1, 1),
                row(
                    "ubuntu",
                    "third",
                    Some("fedcba9876543210fedcba9876543210fedcba98"),
                    1,
                    1,
                ),
            ],
            &o,
        );
        assert_eq!(output.unique_results.len(), 2);
        assert_eq!(output.duplicate_count, 1);
        assert_eq!(
            output
                .unique_results
                .iter()
                .find(|r| r.info_hash.as_deref() == Some(h))
                .unwrap()
                .indexer_instance_id,
            "second"
        );
    }
    #[test]
    fn avoid_title_only_collapse_and_apply_filters() {
        let o = ResultOptions {
            min_seeders: Some(3),
            ..ResultOptions::default()
        };
        let output = process(
            [
                row("same", "a", None, 1, 4),
                row("same", "b", None, 2, 4),
                row("same", "c", None, 1, 0),
            ],
            &o,
        );
        assert_eq!(output.unique_results.len(), 2);
        assert_eq!(output.filtered_count, 1);
    }
}
