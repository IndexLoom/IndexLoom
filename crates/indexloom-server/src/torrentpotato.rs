//! TorrentPotato/CouchPotato-compatible adapter backed by the same safe feed
//! search and durable download tickets as Torznab.
use crate::{AppState, torznab};
use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    routing::get,
};
use indexloom_engine::SearchParameters;
use serde::Deserialize;
use serde_json::{Value, json};
use subtle::ConstantTimeEq;

#[derive(Clone, Default, Deserialize)]
struct PotatoQuery {
    t: Option<String>,
    apikey: Option<String>,
    q: Option<String>,
    imdbid: Option<String>,
    limit: Option<usize>,
    offset: Option<usize>,
    cat: Option<String>,
}
pub(crate) fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/v1/torrentpotato", get(global))
        .route("/api/v1/torrentpotato/{indexer}", get(single))
        .route("/potato", get(global))
}
fn authorized(token: Option<&str>) -> bool {
    let secret = std::env::var("INDEXLOOM_TORRENTPOTATO_API_KEY")
        .or_else(|_| std::env::var("INDEXLOOM_TORZNAB_API_KEY"));
    secret.is_ok_and(|v| {
        (32..=512).contains(&v.len())
            && v.as_bytes()
                .ct_eq(token.unwrap_or("").as_bytes())
                .unwrap_u8()
                == 1
    })
}
async fn global(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<PotatoQuery>,
) -> (StatusCode, Json<Value>) {
    handle(&state, &headers, query, None).await
}
async fn single(
    State(state): State<AppState>,
    Path(indexer): Path<String>,
    headers: HeaderMap,
    Query(query): Query<PotatoQuery>,
) -> (StatusCode, Json<Value>) {
    handle(&state, &headers, query, Some(indexer)).await
}
async fn handle(
    state: &AppState,
    headers: &HeaderMap,
    query: PotatoQuery,
    only: Option<String>,
) -> (StatusCode, Json<Value>) {
    if !authorized(query.apikey.as_deref()) {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({"error":"unauthorized"})),
        );
    }
    if query.t.as_deref().unwrap_or("search") == "caps" {
        return (
            StatusCode::OK,
            Json(json!({"result":"success","search":true,
            "capabilities":{"imdb":true,"q":true,"category":true}})),
        );
    }
    if !matches!(query.t.as_deref().unwrap_or("search"), "search" | "movie") {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error":"unsupported operation"})),
        );
    }
    let offset = query.offset.unwrap_or(0);
    let limit = query.limit.unwrap_or(50);
    if limit == 0
        || limit > 200
        || offset > 200
        || offset.saturating_add(limit) > 200
        || query.q.as_ref().is_some_and(|v| v.len() > 200)
        || query.imdbid.as_ref().is_some_and(|v| v.len() > 100)
    {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error":"invalid search"})),
        );
    }
    let params = SearchParameters {
        mode: Some("movie-search".into()),
        category: query.cat,
        imdb_id: query.imdbid.clone(),
        ..Default::default()
    };
    let term = query.q.or(query.imdbid).unwrap_or_default();
    let results =
        crate::execution::search_for_feed(state, term, params, only, offset + limit).await;
    let Ok(rows) = results else {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error":"search failed"})),
        );
    };
    let total = rows.len();
    let base = torznab::download_base(headers);
    let releases: Vec<_> = rows
        .into_iter()
        .skip(offset)
        .take(limit)
        .filter(|r| r.protocol == indexloom_core::ReleaseProtocol::Torrent)
        .filter_map(|release| {
            let link = release.download_url.as_ref()?;
            let link = if link.starts_with('/') {
                format!("{base}{link}")
            } else {
                link.to_owned()
            };
            Some(json!({
                "release_title":release.title,"release_download":link,
                "release_id":release.id,"release_size":release.size.unwrap_or(0),
                "release_seeders":release.seeders.unwrap_or(0),
                "release_leechers":release.leechers.unwrap_or(0),
                "release_infohash":release.info_hash,
                "release_guid":release.source_guid,
                "release_type":"torrent",
                "release_date":release.published_at,
                "release_imdbid":release.attributes.imdb_id,
                "release_category":release.category,
                "release_quality":"unknown",
            }))
        })
        .collect();
    (
        StatusCode::OK,
        Json(json!({
            "result":"success","total":total,
            "offset":offset,"releases":releases
        })),
    )
}
