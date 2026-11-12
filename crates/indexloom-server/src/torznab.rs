//! Experimental Torznab v1 search/RSS bridge.
//! A separate high-entropy API key is required; the operator token is never accepted.
//! Never put user-supplied search text into XML without escaping.
use axum::{
    Router,
    extract::{Query, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
    routing::get,
};
use indexloom_core::SearchResult;
use serde::Deserialize;
use subtle::ConstantTimeEq;

use crate::AppState;

#[derive(Default, Deserialize)]
struct QueryParams {
    t: Option<String>,
    q: Option<String>,
    apikey: Option<String>,
    limit: Option<usize>,
    offset: Option<usize>,
    cat: Option<String>,
}

pub(crate) fn routes() -> Router<AppState> {
    Router::new().route("/api/v1/torznab", get(torznab))
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
    ).into_response()
}

fn failure(status: StatusCode, code: u16, reason: &str) -> Response {
    xml(status, format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?><error code=\"{code}\" description=\"{}\"/>",
        xml_escape(reason),
    ))
}

async fn torznab(
    State(state): State<AppState>,
    Query(params): Query<QueryParams>,
) -> Response {
    // Disabled by default. NEVER reuse the owner token as a Torznab API key.
    let Ok(secret) = std::env::var("INDEXLOOM_TORZNAB_API_KEY") else {
        return failure(StatusCode::SERVICE_UNAVAILABLE, 101, "Torznab API is disabled");
    };
    if secret.len() < 32 || secret.len() > 512 {
        return failure(StatusCode::SERVICE_UNAVAILABLE, 101, "Invalid server API key configuration");
    }
    let provided = params.apikey.as_deref().unwrap_or("");
    if secret.as_bytes().ct_eq(provided.as_bytes()).unwrap_u8() != 1 {
        return failure(StatusCode::UNAUTHORIZED, 100, "Invalid API key");
    }
    match params.t.as_deref().unwrap_or("caps") {
        "caps" => xml(StatusCode::OK, caps()),
        "search" => {
            let query = params.q.as_deref().unwrap_or("").trim();
            if query.is_empty() {
                return failure(StatusCode::BAD_REQUEST, 200, "Missing search query");
            }
            if params.offset.unwrap_or(0) > 200 {
                return failure(StatusCode::BAD_REQUEST, 201, "Offset exceeds maximum");
            }
            let count = params.limit.unwrap_or(50).clamp(1, 200);
            let offset = params.offset.unwrap_or(0);
            let total = count.saturating_add(offset).min(200);
            // Only a single source category is supported by the bounded Cardigann query.
            let category = params.cat.as_deref().filter(|c| !c.is_empty())
                .and_then(|c| c.split(',').next()).map(str::to_owned);
            let results = match crate::execution::search_for_feed(&state, query.to_owned(), category, total).await {
                Ok(results) => results,
                Err(_) => return failure(StatusCode::BAD_REQUEST, 202, "Search failed or no enabled indexers"),
            };
            xml(StatusCode::OK, rss(&results.into_iter().skip(offset).take(count).collect::<Vec<_>>()))
        },
        _ => failure(StatusCode::BAD_REQUEST, 203, "Unsupported Torznab operation"),
    }
}

fn caps() -> String {
    r#"<?xml version="1.0" encoding="UTF-8"?>
<caps>
  <server version="0.3.0" title="IndexLoom" strapline="Self-hosted indexer manager"/>
  <limits max="200" default="50"/>
  <searching><search available="yes" supportedParams="q,cat"/></searching>
  <categories>
    <category id="2000" name="Movies"/>
    <category id="3000" name="Audio"/>
    <category id="4000" name="PC"/>
    <category id="5000" name="TV"/>
    <category id="7000" name="Books"/>
    <category id="8000" name="Other"/>
  </categories>
</caps>"#.to_owned()
}

fn rss(items: &[SearchResult]) -> String {
    let mut output = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?><rss version=\"2.0\" xmlns:torznab=\"http://torznab.com/schemas/2015/feed\"><channel><title>IndexLoom</title><description>Federated torrent search</description>"
    );
    for item in items {
        let guid = item.source_guid.as_deref()
            .or(item.details_url.as_deref())
            .or(item.source_id.as_deref())
            .unwrap_or("");
        let guid = if guid.is_empty() { item.id.to_string() } else { guid.to_owned() };
        output.push_str("<item><title>");
        output.push_str(&xml_escape(&item.title));
        output.push_str("</title><guid isPermaLink=\"false\">");
        output.push_str(&xml_escape(&guid));
        output.push_str("</guid>");
        if let Some(url) = item.download_url.as_deref().or(item.magnet_url.as_deref()).or(item.details_url.as_deref()) {
            output.push_str("<link>");
            output.push_str(&xml_escape(url));
            output.push_str("</link>");
        }
        if let Some(url) = item.download_url.as_deref() {
            output.push_str("<enclosure url=\"");
            output.push_str(&xml_escape(url));
            output.push_str("\" length=\"");
            output.push_str(&item.size.unwrap_or(0).to_string());
            output.push_str("\" type=\"application/x-bittorrent\"/>");
        }
        if let Some(date) = &item.published_at
            && let Ok(text) = date.format(&time::format_description::well_known::Rfc2822)
        {
            output.push_str("<pubDate>");
            output.push_str(&xml_escape(&text));
            output.push_str("</pubDate>");
        }
        for (key, value) in [
            ("size", item.size.map(|v| v.to_string())),
            ("seeders", item.seeders.map(|v| v.to_string())),
            ("leechers", item.leechers.map(|v| v.to_string())),
            ("infohash", item.info_hash.clone()),
        ] {
            if let Some(value) = value {
                output.push_str("<torznab:attr name=\"");
                output.push_str(key);
                output.push_str("\" value=\"");
                output.push_str(&xml_escape(&value));
                output.push_str("\"/>");
            }
        }
        let category = item.category.as_deref().filter(|value| value.len() == 4 && value.bytes().all(|b| b.is_ascii_digit())).unwrap_or("8000");
        output.push_str("<torznab:attr name=\"category\" value=\"");
        output.push_str(category);
        output.push_str("\"/></item>");
    }
    output.push_str("</channel></rss>");
    output
}

fn xml_escape(value: &str) -> String {
    let mut output = String::with_capacity(value.len());
    for character in value.chars() {
        match character {
            '&' => output.push_str("&amp;"),
            '<' => output.push_str("&lt;"),
            '>' => output.push_str("&gt;"),
            '"' => output.push_str("&quot;"),
            '\'' => output.push_str("&apos;"),
            '\u{9}' | '\u{A}' | '\u{D}' => output.push(character),
            '\u{20}'..='\u{D7FF}' | '\u{E000}'..='\u{FFFD}' | '\u{10000}'..='\u{10FFFF}' => output.push(character),
            _ => {}
        }
    }
    output
}

#[cfg(test)]
mod tests {
    use super::{caps, rss, xml_escape};
    use indexloom_core::SearchResult;
    use uuid::Uuid;
    #[test]
    fn escapes_all_untrusted_xml_content() {
        assert_eq!(xml_escape("привет & <world> \"'\u{1}"), "привет &amp; &lt;world&gt; &quot;&apos;");
        assert!(caps().contains("supportedParams=\"q,cat\""));
    }

    #[test]
    fn rss_preserves_unicode_titles_but_never_injects_markup() {
        let result = SearchResult {
            id: Uuid::nil(),
            indexer_instance_id: "a".into(),
            definition_id: "a".into(),
            source_id: None, source_guid: None, info_hash: None,
            title: "映画 & <anime>".into(),
            details_url: Some("https://example.org/?a=1&b=2".into()),
            download_url: None,
            magnet_url: None,
            category: None,
            size: Some(42), seeders: Some(5), leechers: Some(1),
            published_at: None,
        };
        let document = rss(&[result]);
        assert!(document.contains("映画 &amp; &lt;anime&gt;"));
        assert!(document.contains("https://example.org/?a=1&amp;b=2"));
        assert!(document.contains("name=\"category\" value=\"8000\""));
    }
}
