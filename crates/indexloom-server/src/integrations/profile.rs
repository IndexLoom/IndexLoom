use crate::{ApiError, validate_identifier};
use indexloom_storage::IndexerConfigurationRecord;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Default, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub(super) enum SyncMode {
    #[default]
    Disabled,
    AddOnly,
    FullSync,
}
#[derive(Clone, Default, Debug, Deserialize, Serialize)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Profile {
    pub mode: SyncMode,
    pub indexer_ids: Vec<String>,
    pub tags: Vec<String>,
    pub group_ids: Vec<String>,
    pub categories: Vec<u32>,
    pub feed_origin: Option<String>,
    pub remote_tags: Vec<i64>,
    pub priority: u16,
}
impl Profile {
    pub fn validate(&self) -> Result<(), ApiError> {
        if self.indexer_ids.len() > 256
            || self.tags.len() > 64
            || self.group_ids.len() > 64
            || self.categories.len() > 128
            || self.remote_tags.len() > 64
            || self.remote_tags.iter().any(|x| *x <= 0)
            || self.categories.iter().any(|x| *x < 1000 || *x > 8999)
            || self.priority > 50
        {
            return Err(ApiError::bad_request("invalid integration profile limits"));
        }
        for id in self.indexer_ids.iter().chain(&self.group_ids) {
            validate_identifier(id)?;
        }
        for text in &self.tags {
            if text.is_empty() || text.len() > 1024 || text.chars().any(char::is_control) {
                return Err(ApiError::bad_request("invalid profile field"));
            }
        }
        if let Some(origin) = &self.feed_origin {
            super::origin(origin)?;
        }
        if self.mode != SyncMode::Disabled && self.feed_origin.is_none() {
            return Err(ApiError::bad_request("active sync requires feedOrigin"));
        }
        Ok(())
    }
    pub async fn validate_selectors(&self, state: &crate::AppState) -> Result<(), ApiError> {
        self.validate()?;
        let indexers = state.store.list_indexers(&state.vault).await?;
        let groups = state.store.list_groups().await?;
        if self
            .indexer_ids
            .iter()
            .any(|id| !indexers.iter().any(|i| &i.id == id))
            || self
                .group_ids
                .iter()
                .any(|id| !groups.iter().any(|g| &g.id == id))
            || self
                .tags
                .iter()
                .any(|tag| !indexers.iter().any(|i| i.tags.contains(tag)))
        {
            return Err(ApiError::bad_request(
                "profile selector references an unknown indexer, group or tag",
            ));
        }
        Ok(())
    }
    // Empty dimensions are unrestricted; IDs, tag union, and group union intersect.
    // Disabled and unavailable indexers never participate. Categories constrain the feed.
    pub fn selects(&self, item: &IndexerConfigurationRecord) -> bool {
        selected(
            item.enabled && item.catalog_available,
            &item.id,
            &item.tags,
            &item.group_ids,
            self,
        )
    }
}
fn selected(enabled: bool, id: &str, tags: &[String], groups: &[String], p: &Profile) -> bool {
    enabled
        && (p.indexer_ids.is_empty() || p.indexer_ids.iter().any(|v| v == id))
        && (p.tags.is_empty() || p.tags.iter().any(|v| tags.contains(v)))
        && (p.group_ids.is_empty() || p.group_ids.iter().any(|v| groups.contains(v)))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn dimensions_intersect_and_empty_matches_all() {
        let mut p = Profile::default();
        assert!(selected(true, "a", &[], &[], &p));
        p.indexer_ids = vec!["a".into()];
        p.tags = vec!["x".into(), "y".into()];
        p.group_ids = vec!["g".into()];
        assert!(selected(true, "a", &["y".into()], &["g".into()], &p));
        assert!(!selected(true, "b", &["y".into()], &["g".into()], &p));
        assert!(!selected(true, "a", &[], &["g".into()], &p));
        assert!(!selected(false, "a", &["y".into()], &["g".into()], &p));
    }
    #[test]
    fn profile_validation() {
        let mut p = Profile {
            mode: SyncMode::FullSync,
            ..Profile::default()
        };
        assert!(p.validate().is_err());
        p.feed_origin = Some("http://localhost:3000".into());
        assert!(p.validate().is_ok());
        p.categories = vec![0];
        assert!(p.validate().is_err());
        assert!(serde_json::from_str::<Profile>(r#"{"unexpected":true}"#).is_err());
    }
}
