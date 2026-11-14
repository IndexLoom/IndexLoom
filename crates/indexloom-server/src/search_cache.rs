//! Bounded credential-isolated TTL cache of raw indexer results.
//! Download tickets are minted *after* cache retrieval, never stored as public URLs.
use indexloom_engine::{CardigannExecutor, IndexerSearchBatch, SearchParameters};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::Mutex;

const TTL: Duration = Duration::from_secs(120);
const MAX_ENTRIES: usize = 256;
const MAX_BYTES: usize = 256 * 1024;

#[derive(Clone, Default)]
pub(crate) struct SearchCache {
    entries: Arc<Mutex<HashMap<[u8; 32], Entry>>>,
}
impl std::fmt::Debug for SearchCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SearchCache(<redacted>)")
    }
}
struct Entry {
    created: Instant,
    executor: Arc<CardigannExecutor>,
    batch: IndexerSearchBatch,
}
fn key(
    indexer: &str,
    term: &str,
    limit: usize,
    parameters: &SearchParameters,
    owner: Option<[u8; 32]>,
) -> [u8; 32] {
    let mut digest = Sha256::new();
    digest.update(b"indexloom:search-cache:v1:");
    for bytes in [
        indexer.as_bytes(),
        term.as_bytes(),
        &limit.to_be_bytes(),
        &serde_json::to_vec(parameters).unwrap_or_default(),
    ] {
        digest.update((bytes.len() as u64).to_be_bytes());
        digest.update(bytes);
    }
    digest.update(owner.unwrap_or([0; 32]));
    digest.finalize().into()
}
impl SearchCache {
    pub async fn get(
        &self,
        indexer: &str,
        term: &str,
        limit: usize,
        params: &SearchParameters,
        owner: Option<[u8; 32]>,
        executor: &Arc<CardigannExecutor>,
    ) -> Option<IndexerSearchBatch> {
        let key = key(indexer, term, limit, params, owner);
        let mut entries = self.entries.lock().await;
        entries.retain(|_, row| row.created.elapsed() < TTL);
        entries
            .get(&key)
            .filter(|row| Arc::ptr_eq(&row.executor, executor))
            .map(|row| row.batch.clone())
    }
    #[allow(
        clippy::too_many_arguments,
        reason = "Cache key binds indexer, query, filters and credential isolation."
    )]
    #[allow(
        clippy::too_many_arguments,
        reason = "Credential-isolated cache keys include all search parameters and runtime identity."
    )]
    pub async fn put(
        &self,
        indexer: &str,
        term: &str,
        limit: usize,
        params: &SearchParameters,
        owner: Option<[u8; 32]>,
        executor: Arc<CardigannExecutor>,
        batch: IndexerSearchBatch,
    ) {
        if serde_json::to_vec(&batch).map_or(true, |body| body.len() > MAX_BYTES) {
            return;
        }
        let key = key(indexer, term, limit, params, owner);
        let mut entries = self.entries.lock().await;
        entries.retain(|_, row| row.created.elapsed() < TTL);
        if entries.len() >= MAX_ENTRIES
            && let Some(oldest) = entries
                .iter()
                .min_by_key(|(_, entry)| entry.created)
                .map(|(key, _)| *key)
        {
            entries.remove(&oldest);
        }
        entries.insert(
            key,
            Entry {
                created: Instant::now(),
                executor,
                batch,
            },
        );
    }
    pub async fn clear(&self) {
        self.entries.lock().await.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::key;
    use indexloom_engine::SearchParameters;
    #[test]
    fn keys_isolate_tokens_and_search_parameters() {
        let a = SearchParameters::default();
        let mut b = a.clone();
        b.tvdb_id = Some("111".into());
        assert_ne!(
            key("a", "q", 50, &a, Some([1; 32])),
            key("a", "q", 50, &a, Some([2; 32]))
        );
        assert_ne!(key("a", "q", 50, &a, None), key("a", "q", 50, &b, None));
        assert_ne!(key("a", "q", 50, &a, None), key("b", "q", 50, &a, None));
    }
}
