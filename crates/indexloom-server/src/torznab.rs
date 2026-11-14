//! Torznab and Newznab-compatible RSS endpoints: caps, search, specialized modes and NZB/torrent grabs.
//! Publicly reachable only with an independent high-entropy API key; raw tracker URLs stay encrypted.
use crate::AppState;
use axum::{
    Router,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
    routing::get,
};
use indexloom_core::{ReleaseProtocol, SearchResult};
use indexloom_engine::SearchParameters;
use serde::Deserialize;
use std::fmt::Write;
use subtle::ConstantTimeEq;

#[derive(Clone, Default, Deserialize)]
struct QueryParams {
    t: Option<String>,
    q: Option<String>,
    apikey: Option<String>,
    limit: Option<usize>,
    offset: Option<usize>,
    cat: Option<String>,
    id: Option<String>,
    imdbid: Option<String>,
    tmdbid: Option<String>,
    tvdbid: Option<String>,
    tvmazeid: Option<String>,
    traktid: Option<String>,
    doubanid: Option<String>,
    rid: Option<String>,
    season: Option<u16>,
    ep: Option<String>,
    year: Option<u16>,
    genre: Option<String>,
    artist: Option<String>,
    album: Option<String>,
    author: Option<String>,
    title: Option<String>,
    publisher: Option<String>,
    label: Option<String>,
    track: Option<String>,
    group: Option<String>,
    minsize: Option<u64>,
    maxsize: Option<u64>,
    maxage: Option<u32>,
    o: Option<String>,
}
pub(crate) fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/v1/torznab", get(torznab))
        .route("/api/v1/torznab/{indexer}", get(single))
        .route("/api/v1/indexers/{indexer}/torznab", get(single))
        .route("/api", get(torznab))
        .route("/api/v1/newznab", get(torznab))
        .route("/api/v1/newznab/{indexer}", get(single))
        .route("/api/v1/indexers/{indexer}/newznab", get(single))
}
fn xml(status: StatusCode, body: String) -> Response {
    (
        status,
        [
            (header::CONTENT_TYPE, "application/xml; charset=utf-8"),
            (header::CACHE_CONTROL, "private, no-store"),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
        ],
        body,
    )
        .into_response()
}
fn failure(status: StatusCode, code: u16, description: &str) -> Response {
    xml(
        status,
        format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?><error code=\"{code}\" description=\"{}\"/>",
            escape(description)
        ),
    )
}
fn authorized(params: &QueryParams) -> bool {
    let Ok(secret) = std::env::var("INDEXLOOM_TORZNAB_API_KEY") else {
        return false;
    };
    if !(32..=512).contains(&secret.len()) {
        return false;
    }
    secret
        .as_bytes()
        .ct_eq(params.apikey.as_deref().unwrap_or("").as_bytes())
        .unwrap_u8()
        == 1
}
async fn torznab(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(params): Query<QueryParams>,
) -> Response {
    handle(&state, &headers, params, None).await
}
async fn single(
    State(state): State<AppState>,
    Path(indexer): Path<String>,
    headers: HeaderMap,
    Query(params): Query<QueryParams>,
) -> Response {
    handle(&state, &headers, params, Some(indexer)).await
}
fn parameters(params: &QueryParams, mode: &str) -> SearchParameters {
    SearchParameters {
        mode: Some(mode.to_owned()),
        category: params.cat.clone(),
        genre: params.genre.clone(),
        year: params.year,
        season: params.season,
        episode: params.ep.as_ref().and_then(|v| v.parse::<u16>().ok()),
        episode_code: params.ep.clone(),
        imdb_id: params.imdbid.clone(),
        tmdb_id: params.tmdbid.clone(),
        tvdb_id: params.tvdbid.clone(),
        tvmaze_id: params.tvmazeid.clone(),
        trakt_id: params.traktid.clone(),
        douban_id: params.doubanid.clone(),
        tvrage_id: params.rid.clone(),
        artist: params.artist.clone(),
        album: params.album.clone(),
        author: params.author.clone(),
        title: params.title.clone(),
        publisher: params.publisher.clone(),
        label: params.label.clone(),
        track: params.track.clone(),
        group: params.group.clone(),
        max_age_days: params.maxage,
        min_size: params.minsize,
        max_size: params.maxsize,
        ..Default::default()
    }
}
pub(crate) fn download_base(headers: &HeaderMap) -> String {
    if let Ok(configured) = std::env::var("INDEXLOOM_PUBLIC_URL")
        && let Ok(url) = url::Url::parse(&configured)
        && matches!(url.scheme(), "https" | "http")
        && url.username().is_empty()
        && url.password().is_none()
        && url.host_str().is_some()
        && url.query().is_none()
        && url.fragment().is_none()
    {
        return configured.trim_end_matches('/').to_owned();
    }
    let host = headers
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if !host.is_empty()
        && host.len() <= 255
        && host
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b':' | b'[' | b']'))
        && host.contains('.')
    {
        return format!("https://{host}");
    }
    String::new()
}
#[allow(
    clippy::too_many_lines,
    reason = "Single compatibility route validates parameters and dispatches restricted search modes."
)]
async fn handle(
    state: &AppState,
    headers: &HeaderMap,
    params: QueryParams,
    only: Option<String>,
) -> Response {
    if !authorized(&params) {
        return failure(
            StatusCode::UNAUTHORIZED,
            100,
            "Invalid API key or Torznab is disabled",
        );
    }
    if params.o.as_deref().is_some_and(|v| v != "xml") {
        return failure(StatusCode::BAD_REQUEST, 201, "Only XML output is supported");
    }
    let op = params.t.as_deref().unwrap_or("caps");
    if op == "caps" {
        if let Some(id) = only.as_deref() {
            if id.len() > 100 {
                return failure(StatusCode::BAD_REQUEST, 204, "Invalid indexer identifier");
            }
            let Ok(record) = state.store.get_indexer(id, &state.vault).await else {
                return failure(StatusCode::NOT_FOUND, 300, "Indexer not found");
            };
            if !record.enabled || !record.catalog_available {
                return failure(StatusCode::NOT_FOUND, 300, "Indexer is unavailable");
            }
            let Ok(definition) = indexloom_definitions::parse_yaml(&record.definition_yaml) else {
                return failure(
                    StatusCode::SERVICE_UNAVAILABLE,
                    202,
                    "Indexer definition invalid",
                );
            };
            return xml(
                StatusCode::OK,
                indexer_caps(&indexloom_definitions::capabilities::summarize(&definition)),
            );
        }
        return xml(StatusCode::OK, caps());
    }
    if op == "get" || op == "download" {
        let guid = params.id.as_deref().unwrap_or("");
        if guid.len() != 64 || !guid.bytes().all(|b| b.is_ascii_hexdigit()) {
            return failure(StatusCode::BAD_REQUEST, 200, "Invalid download token");
        }
        let Ok(Some(token)) = state.store.download_ticket_for_guid(guid).await else {
            return failure(
                StatusCode::NOT_FOUND,
                300,
                "Download GUID expired or unknown",
            );
        };
        let mut response = StatusCode::TEMPORARY_REDIRECT.into_response();
        response.headers_mut().insert(
            header::LOCATION,
            format!("/api/v1/dl/{token}").parse().unwrap(),
        );
        response
            .headers_mut()
            .insert(header::CACHE_CONTROL, "no-store".parse().unwrap());
        return response;
    }
    let mode = match op {
        "search" => "search",
        "tvsearch" | "tv-search" => "tv-search",
        "movie" | "moviesearch" => "movie-search",
        "music" | "audio" | "audiosearch" => "music-search",
        "book" | "booksearch" => "book-search",
        _ => {
            return failure(
                StatusCode::BAD_REQUEST,
                203,
                "Unsupported Torznab search operation",
            );
        }
    };
    if let Some(indexer) = only.as_deref()
        && (indexer.len() > 100
            || !indexer
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-'))
    {
        return failure(StatusCode::BAD_REQUEST, 204, "Invalid indexer id");
    }
    let offset = params.offset.unwrap_or(0);
    let limit = params.limit.unwrap_or(50);
    if offset > 200 || limit == 0 || limit > 200 || offset.saturating_add(limit) > 200 {
        return failure(
            StatusCode::BAD_REQUEST,
            201,
            "Invalid result limit or offset",
        );
    }
    let fields = [
        params.q.as_deref(),
        params.cat.as_deref(),
        params.imdbid.as_deref(),
        params.tmdbid.as_deref(),
        params.tvdbid.as_deref(),
        params.tvmazeid.as_deref(),
        params.traktid.as_deref(),
        params.doubanid.as_deref(),
        params.rid.as_deref(),
        params.ep.as_deref(),
        params.genre.as_deref(),
        params.artist.as_deref(),
        params.album.as_deref(),
        params.author.as_deref(),
        params.title.as_deref(),
        params.publisher.as_deref(),
        params.group.as_deref(),
    ];
    if fields
        .into_iter()
        .flatten()
        .any(|v| v.len() > 200 || v.chars().any(char::is_control))
    {
        return failure(StatusCode::BAD_REQUEST, 200, "Invalid search parameter");
    }
    let parameters = parameters(&params, mode);
    let term = params
        .q
        .clone()
        .or(params.title.clone())
        .or(params.artist.clone())
        .or(params.author.clone())
        .unwrap_or_default();
    let results =
        crate::execution::search_for_feed(state, term, parameters, only, offset + limit).await;
    let Ok(rows) = results else {
        return failure(
            StatusCode::BAD_REQUEST,
            202,
            "Search failed or no compatible enabled indexers",
        );
    };
    let count = rows.len();
    let base = download_base(headers);
    let selected = rows
        .into_iter()
        .skip(offset)
        .take(limit)
        .collect::<Vec<_>>();
    xml(StatusCode::OK, rss(&selected, offset, count, &base))
}
fn caps() -> String {
    String::from(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<caps><server version="0.3.0" title="IndexLoom" strapline="Federated Torrent/Usenet search"/>
<limits max="200" default="50"/>
<retention default="0"/><registration available="no" open="no"/>
<searching>
<search available="yes" supportedParams="q,cat"/>
<tv-search available="yes" supportedParams="q,cat,season,ep,imdbid,tvdbid,tvmazeid,rid,traktid"/>
<movie-search available="yes" supportedParams="q,cat,imdbid,tmdbid,doubanid,year"/>
<audio-search available="yes" supportedParams="q,cat,artist,album,label,track"/>
<book-search available="yes" supportedParams="q,cat,author,title,publisher"/>
</searching>
<categories><category id="2000" name="Movies"/><category id="3000" name="Audio"/>
<category id="4000" name="PC"/><category id="5000" name="TV"/>
<category id="6000" name="XXX"/><category id="7000" name="Books"/>
<category id="8000" name="Other"/></categories></caps>"#,
    )
}
fn indexer_caps(indexer: &indexloom_definitions::capabilities::IndexerCapabilities) -> String {
    let mut body = caps();
    let mode_names = indexer
        .search_modes
        .iter()
        .map(|m| m.name.as_str())
        .collect::<Vec<_>>();
    for (xml_mode, variants) in [
        ("search", &["search"][..]),
        ("tv-search", &["tv-search", "tvsearch"][..]),
        (
            "movie-search",
            &["movie-search", "movie", "moviesearch"][..],
        ),
        (
            "audio-search",
            &["music-search", "music", "audio", "audiosearch"][..],
        ),
        ("book-search", &["book-search", "book", "booksearch"][..]),
    ] {
        if !variants.iter().any(|mode| mode_names.contains(mode)) {
            body = body.replace(
                &format!("<{xml_mode} available=\"yes\""),
                &format!("<{xml_mode} available=\"no\""),
            );
        }
    }
    let category_roots = indexer
        .categories
        .iter()
        .filter_map(|cat| cat.split('/').next())
        .collect::<std::collections::HashSet<_>>();
    if !category_roots.is_empty() {
        for (code, root) in [
            ("1000", "Console"),
            ("2000", "Movies"),
            ("3000", "Audio"),
            ("4000", "PC"),
            ("5000", "TV"),
            ("6000", "XXX"),
            ("7000", "Books"),
            ("8000", "Other"),
        ] {
            if !category_roots.contains(root) {
                body = body.replace(&format!("<category id=\"{code}\" name=\"{root}\"/>"), "");
            }
        }
    }
    body
}
fn escape(value: &str) -> String {
    let mut output = String::with_capacity(value.len());
    for c in value.chars() {
        match c {
            '&' => output.push_str("&amp;"),
            '<' => output.push_str("&lt;"),
            '>' => output.push_str("&gt;"),
            '"' => output.push_str("&quot;"),
            '\'' => output.push_str("&apos;"),
            '\u{9}' | '\u{A}' | '\u{D}' => output.push(c),
            '\u{20}'..='\u{D7FF}' | '\u{E000}'..='\u{FFFD}' | '\u{10000}'..='\u{10FFFF}' => {
                output.push(c);
            }
            _ => {}
        }
    }
    output
}
fn attr(out: &mut String, name: &str, value: &str) {
    if value.is_empty() || value.len() > 2048 {
        return;
    }
    // Some clients expect the Torznab namespace; others only parse Newznab.
    // Both carry the same sanitized values to keep one result model.
    for namespace in ["torznab", "newznab"] {
        out.push('<');
        out.push_str(namespace);
        out.push_str(":attr name=\"");
        out.push_str(name);
        out.push_str("\" value=\"");
        out.push_str(&escape(value));
        out.push_str("\"/>");
    }
}
fn opt_attr(out: &mut String, key: &str, value: Option<String>) {
    if let Some(value) = value {
        attr(out, key, &value);
    }
}
#[allow(
    clippy::too_many_lines,
    reason = "Bounded RSS serialization keeps extension attributes encoded consistently."
)]
fn rss(items: &[SearchResult], offset: usize, total: usize, base: &str) -> String {
    let mut output = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\"?>");
    output.push_str("<rss version=\"2.0\" xmlns:torznab=\"http://torznab.com/schemas/2015/feed\" ");
    output
        .push_str("xmlns:newznab=\"http://www.newznab.com/DTD/2010/feeds/attributes/\"><channel>");
    output.push_str(
        "<title>IndexLoom</title><description>Federated Torrent and Usenet Search</description>",
    );
    let _ = write!(
        output,
        "<newznab:response offset=\"{offset}\" total=\"{total}\"/>"
    );
    for item in items {
        let link = item.download_url.as_deref().unwrap_or("");
        if link.is_empty() {
            continue;
        }
        let url = if link.starts_with('/') {
            format!("{base}{link}")
        } else {
            link.to_owned()
        };
        output.push_str("<item><title>");
        output.push_str(&escape(&item.title));
        output.push_str("</title>");
        output.push_str("<guid isPermaLink=\"false\">");
        output.push_str(&escape(
            item.source_guid
                .as_deref()
                .or(item.info_hash.as_deref())
                .unwrap_or(&item.id.to_string()),
        ));
        output.push_str("</guid><link>");
        output.push_str(&escape(&url));
        output.push_str("</link>");
        output.push_str("<enclosure url=\"");
        output.push_str(&escape(&url));
        output.push_str("\" length=\"");
        output.push_str(&item.size.unwrap_or(0).to_string());
        output.push_str("\" type=\"");
        output.push_str(if item.protocol == ReleaseProtocol::Usenet {
            "application/x-nzb"
        } else {
            "application/x-bittorrent"
        });
        output.push_str("\"/>");
        if let Some(date) = item.published_at
            && let Ok(s) = date.format(&time::format_description::well_known::Rfc2822)
        {
            output.push_str("<pubDate>");
            output.push_str(&escape(&s));
            output.push_str("</pubDate>");
        }
        opt_attr(&mut output, "size", item.size.map(|n| n.to_string()));
        opt_attr(&mut output, "seeders", item.seeders.map(|n| n.to_string()));
        opt_attr(
            &mut output,
            "leechers",
            item.leechers.map(|n| n.to_string()),
        );
        opt_attr(
            &mut output,
            "peers",
            Some((item.seeders.unwrap_or(0) + item.leechers.unwrap_or(0)).to_string()),
        );
        opt_attr(&mut output, "infohash", item.info_hash.clone());
        let a = &item.attributes;
        opt_attr(&mut output, "files", a.files.map(|n| n.to_string()));
        opt_attr(&mut output, "grabs", a.grabs.map(|n| n.to_string()));
        for (key, value) in [
            ("poster", a.poster.as_deref()),
            ("artist", a.artist.as_deref()),
            ("album", a.album.as_deref()),
            ("author", a.author.as_deref()),
            ("publisher", a.publisher.as_deref()),
            ("group", a.group.as_deref()),
            ("uploader", a.uploader.as_deref()),
            ("imdbid", a.imdb_id.as_deref()),
            ("tmdbid", a.tmdb_id.as_deref()),
            ("tvdbid", a.tvdb_id.as_deref()),
            ("tvmazeid", a.tvmaze_id.as_deref()),
            ("traktid", a.trakt_id.as_deref()),
            ("doubanid", a.douban_id.as_deref()),
            ("rid", a.tvrage_id.as_deref()),
        ] {
            if let Some(value) = value {
                attr(&mut output, key, value);
            }
        }
        for (key, values) in [
            ("genre", &a.genres),
            ("language", &a.languages),
            ("subtitles", &a.subtitles),
        ] {
            for value in values.iter().take(16) {
                attr(&mut output, key, value);
            }
        }
        for (key, num) in [
            ("downloadvolumefactor", a.download_volume_factor),
            ("uploadvolumefactor", a.upload_volume_factor),
            ("minimumratio", a.minimum_ratio),
        ] {
            if let Some(n) = num.filter(|n| n.is_finite()) {
                attr(&mut output, key, &n.to_string());
            }
        }
        opt_attr(
            &mut output,
            "minimumseedtime",
            a.minimum_seed_time.map(|n| n.to_string()),
        );
        let source_category = item.category.as_deref().unwrap_or("");
        let category =
            if source_category.len() == 4 && source_category.bytes().all(|b| b.is_ascii_digit()) {
                source_category
            } else {
                match source_category.split('/').next().unwrap_or(source_category) {
                    "Console" => "1000",
                    "Movies" => "2000",
                    "Audio" => "3000",
                    "PC" => "4000",
                    "TV" => "5000",
                    "XXX" => "6000",
                    "Books" => "7000",
                    _ => "8000",
                }
            };
        attr(&mut output, "category", category);
        output.push_str("</item>");
    }
    output.push_str("</channel></rss>");
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;
    #[test]
    fn sanitizes_untrusted_rss_and_enclosure_protocol() {
        let mut result = SearchResult {
            id: Uuid::nil(),
            indexer_instance_id: "sample".into(),
            definition_id: "sample".into(),
            source_id: None,
            source_guid: Some("guid".into()),
            info_hash: None,
            title: "A & <script>".into(),
            details_url: None,
            download_url: Some("/api/v1/dl/abc".into()),
            magnet_url: None,
            category: Some("5000".into()),
            size: Some(42),
            seeders: Some(2),
            leechers: Some(1),
            published_at: None,
            protocol: ReleaseProtocol::Torrent,
            attributes: indexloom_core::ReleaseAttributes::default(),
        };
        result.attributes.files = Some(5);
        let rss = rss(&[result], "0".parse().unwrap(), 1, "https://example.org");
        assert!(rss.contains("A &amp; &lt;script&gt;"));
        assert!(rss.contains("name=\"files\" value=\"5\""));
        assert!(rss.contains("https://example.org/api/v1/dl/abc"));
        assert!(caps().contains("tv-search"));
    }
}
