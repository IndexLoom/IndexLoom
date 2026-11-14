use crate::{ApiError, AppState};
use sha2::{Digest, Sha256};

pub(super) async fn scan(state: &AppState, id: &str) -> Result<usize, ApiError> {
    let indexer = state.store.get_indexer(id, &state.vault).await?;
    if !indexer.enabled || !indexer.catalog_available {
        return Err(ApiError::conflict("RSS indexer is disabled or unavailable"));
    }
    let entries = crate::execution::scan_indexer(state, id).await?;
    let entries: Vec<_> = entries
        .into_iter()
        .map(|r| {
            let mut digest = Sha256::new();
            for part in [
                r.source_guid
                    .as_deref()
                    .or(r.source_id.as_deref())
                    .or(r.info_hash.as_deref())
                    .unwrap_or(&r.title),
                &r.title,
            ] {
                digest.update((part.len() as u64).to_be_bytes());
                digest.update(part.as_bytes());
            }
            digest.update(r.size.unwrap_or(0).to_be_bytes());
            (format!("{:x}", digest.finalize()), r.title)
        })
        .collect();
    Ok(state.store.record_rss_scan(id, &entries).await?)
}
