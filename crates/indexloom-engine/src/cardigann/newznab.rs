//! Native Newznab client. Bounded XML and NZB parsing shares the origin-gated
//! HTTP transport, per-indexer pacing and encrypted credentials of Cardigann.
use super::{
    CardigannExecutor, EngineError, IndexerSearchBatch, Instant, Method, RequestSpec,
    SearchParameters, Url, elapsed_millis,
};
use indexloom_core::{ReleaseAttributes, ReleaseProtocol, SearchResult};
use quick_xml::{Reader, XmlVersion, events::Event};
use std::collections::BTreeMap;
use time::{OffsetDateTime, format_description::well_known::Rfc2822};
use uuid::Uuid;

fn protocol_error() -> EngineError {
    EngineError::CardigannIncompatible("invalid or oversized Newznab response")
}

fn custom_api_url(base: &Url) -> Result<Url, EngineError> {
    if base.path().trim_end_matches('/').ends_with("/api") {
        Ok(base.clone())
    } else {
        base.join("api").map_err(EngineError::from)
    }
}
fn api_key(executor: &CardigannExecutor) -> Result<String, EngineError> {
    let value = executor
        .context
        .config
        .get("apiKey")
        .and_then(|v| match v {
            super::TemplateValue::String(s) => Some(s.as_str()),
            _ => None,
        })
        .filter(|s| (8..=512).contains(&s.len()) && !s.chars().any(char::is_control))
        .ok_or(EngineError::AuthenticationRejected)?;
    Ok(value.to_owned())
}
fn newznab_category(category: &str) -> String {
    let normalize = |value: &str| {
        match value.split('/').next().unwrap_or(value) {
            "Movies" => "2000",
            "TV" => "5000",
            "Audio" => "3000",
            "Books" => "7000",
            "PC" => "4000",
            "Other" => "8000",
            "XXX" => "6000",
            "Console" => "1000",
            _ => value,
        }
        .to_owned()
    };
    category
        .split(',')
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .take(32)
        .map(normalize)
        .collect::<Vec<_>>()
        .join(",")
}
fn category_label(value: &str) -> String {
    match value.parse::<u16>().ok().map(|n| n / 1000) {
        Some(1) => "Console",
        Some(2) => "Movies",
        Some(3) => "Audio",
        Some(4) => "PC",
        Some(5) => "TV",
        Some(6) => "XXX",
        Some(7) => "Books",
        Some(8) => "Other",
        _ => value,
    }
    .to_owned()
}
fn query_url(
    executor: &CardigannExecutor,
    base: &Url,
    term: &str,
    params: &SearchParameters,
    offset: usize,
    limit: usize,
    t: &str,
) -> Result<Url, EngineError> {
    let mut url = custom_api_url(base)?;
    if url.query().is_some() || url.fragment().is_some() {
        return Err(EngineError::OutboundAddressRejected);
    }
    {
        let mut query = url.query_pairs_mut();
        query.append_pair("t", t);
        query.append_pair("apikey", &api_key(executor)?);
        if !term.is_empty() {
            query.append_pair("q", term);
        }
        query.append_pair("limit", &limit.clamp(1, 200).to_string());
        query.append_pair("offset", &offset.min(2000).to_string());
        for (key, val) in [
            ("genre", params.genre.as_deref()),
            ("imdbid", params.imdb_id.as_deref()),
            ("tmdbid", params.tmdb_id.as_deref()),
            ("tvdbid", params.tvdb_id.as_deref()),
            ("tvmazeid", params.tvmaze_id.as_deref()),
            ("traktid", params.trakt_id.as_deref()),
            ("doubanid", params.douban_id.as_deref()),
            ("rid", params.tvrage_id.as_deref()),
            ("artist", params.artist.as_deref()),
            ("album", params.album.as_deref()),
            ("author", params.author.as_deref()),
            ("title", params.title.as_deref()),
            ("publisher", params.publisher.as_deref()),
            ("label", params.label.as_deref()),
            ("track", params.track.as_deref()),
            ("group", params.group.as_deref()),
        ]
        .into_iter()
        .filter_map(|(key, v)| v.map(|v| (key, v)))
        {
            query.append_pair(key, val);
        }
        if let Some(cat) = params.category.as_deref() {
            let mapped = newznab_category(cat);
            if !mapped.is_empty() {
                query.append_pair("cat", &mapped);
            }
        }
        if let Some(year) = params.year {
            query.append_pair("year", &year.to_string());
        }
        if let Some(season) = params.season {
            query.append_pair("season", &season.to_string());
        }
        if let Some(episode) = params.episode_code.as_deref() {
            query.append_pair("ep", episode);
        } else if let Some(episode) = params.episode {
            query.append_pair("ep", &episode.to_string());
        }
        if let Some(age) = params.max_age_days {
            query.append_pair("maxage", &age.to_string());
        }
        if let Some(size) = params.min_size {
            query.append_pair("minsize", &size.to_string());
        }
        if let Some(size) = params.max_size {
            query.append_pair("maxsize", &size.to_string());
        }
        query.append_pair("extended", "1");
    }
    Ok(url)
}

#[derive(Default)]
struct Item {
    fields: BTreeMap<String, String>,
    attributes: BTreeMap<String, String>,
    enclosure: Option<String>,
}
fn attr(
    e: &quick_xml::events::BytesStart<'_>,
    name: &[u8],
    reader: &Reader<&[u8]>,
) -> Result<Option<String>, EngineError> {
    for attribute in e.attributes() {
        let attribute = attribute.map_err(|_| protocol_error())?;
        if attribute.key.as_ref() == name {
            let decoded = attribute
                .decoded_and_normalized_value(XmlVersion::Implicit1_0, reader.decoder())
                .map_err(|_| protocol_error())?;
            return Ok(Some(decoded.into_owned()));
        }
    }
    Ok(None)
}
fn normalized_link(base: &Url, value: &str) -> Option<String> {
    let parsed = base.join(value).ok()?;
    if parsed.scheme() != base.scheme()
        || parsed.host_str() != base.host_str()
        || parsed.port_or_known_default() != base.port_or_known_default()
        || parsed.username() != ""
        || parsed.password().is_some()
    {
        return None;
    }
    Some(parsed.to_string())
}
fn field<'a>(fields: &'a BTreeMap<String, String>, name: &str) -> Option<&'a str> {
    fields
        .get(name)
        .map(String::as_str)
        .filter(|v| !v.is_empty())
}
fn make_result(base: &Url, instance: &str, definition: &str, item: &Item) -> Option<SearchResult> {
    let title = field(&item.fields, "title")?.to_owned();
    if title.len() > 2048 || title.chars().any(char::is_control) {
        return None;
    }
    let url = item
        .enclosure
        .as_deref()
        .or_else(|| field(&item.fields, "link"))
        .and_then(|v| normalized_link(base, v));
    let details = field(&item.fields, "comments")
        .or_else(|| field(&item.fields, "link"))
        .and_then(|v| normalized_link(base, v));
    url.as_ref()?;
    let mut attributes = ReleaseAttributes::default();
    let text = |key| item.attributes.get(key).filter(|v| v.len() < 512).cloned();
    let number = |key| text(key).and_then(|n: String| n.parse::<u64>().ok());
    attributes.files = number("files").and_then(|n| u32::try_from(n).ok());
    attributes.grabs = number("grabs").or_else(|| number("downloads"));
    attributes.group = text("group");
    attributes.uploader = text("poster");
    attributes.artist = text("artist");
    attributes.album = text("album");
    attributes.author = text("author");
    attributes.publisher = text("publisher");
    attributes.imdb_id = text("imdbid");
    attributes.tmdb_id = text("tmdbid");
    attributes.tvdb_id = text("tvdbid");
    attributes.tvmaze_id = text("tvmazeid");
    attributes.trakt_id = text("traktid");
    attributes.douban_id = text("doubanid");
    attributes.tvrage_id = text("rid");
    attributes.genres = text("genre")
        .map(|v| {
            v.split(',')
                .map(str::trim)
                .take(16)
                .map(ToOwned::to_owned)
                .collect()
        })
        .unwrap_or_default();
    attributes.languages = text("language")
        .map(|v| {
            v.split(',')
                .map(str::trim)
                .take(16)
                .map(ToOwned::to_owned)
                .collect()
        })
        .unwrap_or_default();
    attributes.subtitles = text("subtitles")
        .map(|v| {
            v.split(',')
                .map(str::trim)
                .take(16)
                .map(ToOwned::to_owned)
                .collect()
        })
        .unwrap_or_default();
    let size = field(&item.fields, "size")
        .or_else(|| field(&item.attributes, "size"))
        .and_then(|s| s.parse::<u64>().ok());
    let date = field(&item.fields, "pubdate").and_then(|s| OffsetDateTime::parse(s, &Rfc2822).ok());
    let source_guid = field(&item.fields, "guid")
        .filter(|s| s.len() <= 1024)
        .map(ToOwned::to_owned);
    Some(SearchResult {
        id: Uuid::now_v7(),
        indexer_instance_id: instance.to_owned(),
        definition_id: definition.to_owned(),
        source_id: source_guid.clone(),
        source_guid,
        info_hash: None,
        title,
        details_url: details,
        download_url: url,
        magnet_url: None,
        category: field(&item.attributes, "category")
            .or_else(|| field(&item.fields, "category"))
            .map(category_label),
        size,
        seeders: None,
        leechers: None,
        published_at: date,
        protocol: ReleaseProtocol::Usenet,
        attributes,
    })
}

/// Handle Newznab attribute/enclosure elements in both empty and start forms.
fn apply_item_attribute(
    reader: &Reader<&[u8]>,
    e: &quick_xml::events::BytesStart<'_>,
    name: &str,
    item: &mut Item,
) -> Result<bool, EngineError> {
    if name == "enclosure" {
        item.enclosure = attr(e, b"url", reader)?.filter(|v| v.len() <= 2048);
        if let Some(length) = attr(e, b"length", reader)? {
            item.fields.insert("size".to_owned(), length);
        }
        return Ok(true);
    }
    if name.ends_with(":attr") || name == "attr" {
        let key = attr(e, b"name", reader)?;
        let val = attr(e, b"value", reader)?;
        if let (Some(key), Some(val)) = (key, val)
            && key.len() <= 48
            && val.len() <= 2048
        {
            item.attributes.insert(key.to_ascii_lowercase(), val);
        }
        return Ok(true);
    }
    Ok(false)
}

/// Read a bounded Newznab RSS document. Never follow external entities or links.
#[allow(
    clippy::too_many_lines,
    reason = "Bounded state-machine parsing keeps RSS item and enclosure handling explicit."
)]
pub(super) fn parse_rss(
    body: &str,
    base: &Url,
    instance: &str,
    definition: &str,
) -> Result<Vec<SearchResult>, EngineError> {
    if body.len() > 16 * 1024 * 1024 {
        return Err(protocol_error());
    }
    let mut reader = Reader::from_str(body);
    let mut current: Option<Item> = None;
    let mut active_tag = String::new();
    let mut active_text = String::new();
    let mut results = Vec::new();
    let mut events = 0;
    let mut depth = 0usize;
    loop {
        let ev = reader.read_event().map_err(|_| protocol_error())?;
        events += 1;
        if events > 250_000 || depth > 64 {
            return Err(protocol_error());
        }
        match ev {
            Event::Start(e) => {
                depth += 1;
                let name = String::from_utf8_lossy(e.name().as_ref()).to_ascii_lowercase();
                if name == "item" {
                    current = Some(Item::default());
                } else if let Some(item) = current.as_mut()
                    && !apply_item_attribute(&reader, &e, &name, item)?
                {
                    active_tag = name;
                    active_text.clear();
                }
            }
            Event::Empty(e) => {
                let name = String::from_utf8_lossy(e.name().as_ref()).to_ascii_lowercase();
                if let Some(item) = current.as_mut() {
                    apply_item_attribute(&reader, &e, &name, item)?;
                }
            }
            Event::Text(t) => {
                if current.is_some() && !active_tag.is_empty() {
                    let text = t
                        .xml_content(XmlVersion::Implicit1_0)
                        .map_err(|_| protocol_error())?;
                    if active_text.len() + text.len() > 8192 {
                        return Err(protocol_error());
                    }
                    active_text.push_str(&text);
                }
            }
            Event::CData(t) => {
                if current.is_some() && !active_tag.is_empty() {
                    let text = t
                        .xml_content(XmlVersion::Implicit1_0)
                        .map_err(|_| protocol_error())?;
                    if active_text.len() + text.len() > 8192 {
                        return Err(protocol_error());
                    }
                    active_text.push_str(&text);
                }
            }
            Event::End(e) => {
                depth = depth.saturating_sub(1);
                let name = String::from_utf8_lossy(e.name().as_ref()).to_ascii_lowercase();
                if name == "item" {
                    if let Some(item) = current.take()
                        && let Some(result) = make_result(base, instance, definition, &item)
                    {
                        results.push(result);
                    }
                    if results.len() > 2000 {
                        return Err(protocol_error());
                    }
                } else if name == active_tag {
                    if let Some(item) = current.as_mut() {
                        item.fields
                            .insert(name.clone(), active_text.trim().to_owned());
                    }
                    active_tag.clear();
                    active_text.clear();
                }
            }
            Event::DocType(_) => return Err(protocol_error()),
            Event::Eof => break,
            _ => {}
        }
    }
    Ok(results)
}

pub(super) fn validate_nzb(bytes: &[u8]) -> Result<(), EngineError> {
    if bytes.len() > 16 * 1024 * 1024 {
        return Err(EngineError::InvalidNzb);
    }
    let body = std::str::from_utf8(bytes).map_err(|_| EngineError::InvalidNzb)?;
    let mut reader = Reader::from_str(body);
    let mut root = false;
    let mut files = 0usize;
    let mut events = 0usize;
    loop {
        let ev = reader.read_event().map_err(|_| EngineError::InvalidNzb)?;
        events += 1;
        if events > 500_000 {
            return Err(EngineError::InvalidNzb);
        }
        match ev {
            Event::Start(e) | Event::Empty(e) => {
                if !root {
                    if e.name().as_ref() != b"nzb" {
                        return Err(EngineError::InvalidNzb);
                    }
                    root = true;
                }
                if e.name().as_ref() == b"file" {
                    files += 1;
                }
            }
            Event::DocType(e) => {
                let text = e.decode().map_err(|_| EngineError::InvalidNzb)?;
                if !text.starts_with("nzb") || text.contains('[') || text.contains("ENTITY") {
                    return Err(EngineError::InvalidNzb);
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    if root && files > 0 {
        Ok(())
    } else {
        Err(EngineError::InvalidNzb)
    }
}

impl CardigannExecutor {
    pub(super) async fn search_newznab(
        &self,
        term: &str,
        limit: usize,
        offset: usize,
        params: &SearchParameters,
    ) -> Result<IndexerSearchBatch, EngineError> {
        let started = Instant::now();
        let mode = match params.mode.as_deref().unwrap_or("search") {
            "tvsearch" | "tv-search" => "tvsearch",
            "movie" | "movie-search" => "movie",
            "music" | "audio" | "music-search" => "music",
            "book" | "book-search" => "book",
            _ => "search",
        };
        let url = query_url(self, &self.base_url, term, params, offset, limit, mode)?;
        let response = self
            .execute_binary(
                RequestSpec {
                    method: Method::GET,
                    url,
                    headers: Vec::new(),
                    inputs: Vec::new(),
                    raw_query: None,
                },
                false,
            )
            .await?;
        if !response.status.is_success() {
            return Err(EngineError::UnexpectedStatus(response.status.as_u16()));
        }
        let body = self.encoding.decode(&response.body)?;
        if body.contains("<error") {
            return Err(EngineError::AuthenticationRejected);
        }
        let mut results = parse_rss(
            &body,
            &self.base_url,
            &self.indexer_instance_id,
            &self.definition.metadata.id,
        )?;
        results.truncate(limit.clamp(1, 200));
        Ok(IndexerSearchBatch {
            indexer_instance_id: self.indexer_instance_id.clone(),
            definition_id: self.definition.metadata.id.clone(),
            indexer_name: self.definition.metadata.name.clone(),
            response_time_ms: elapsed_millis(started),
            results,
        })
    }

    pub(super) async fn test_newznab(&self) -> Result<String, EngineError> {
        let mut url = custom_api_url(&self.base_url)?;
        url.query_pairs_mut()
            .append_pair("t", "caps")
            .append_pair("apikey", &api_key(self)?);
        let response = self
            .execute_binary(
                RequestSpec {
                    method: Method::GET,
                    url,
                    headers: Vec::new(),
                    inputs: Vec::new(),
                    raw_query: None,
                },
                false,
            )
            .await?;
        if !response.status.is_success() {
            return Err(EngineError::UnexpectedStatus(response.status.as_u16()));
        }
        let body = self.encoding.decode(&response.body)?;
        if body.contains("<caps") {
            Ok("Newznab capability query succeeded".to_owned())
        } else {
            Err(protocol_error())
        }
    }

    pub(super) async fn download_newznab(
        &self,
        release: &SearchResult,
    ) -> Result<Vec<u8>, EngineError> {
        let raw = release
            .download_url
            .as_ref()
            .ok_or(EngineError::DownloadNotFound)?;
        let url = Url::parse(raw)?;
        self.ensure_allowed(&url)?;
        let response = self
            .execute_binary(
                RequestSpec {
                    method: Method::GET,
                    url,
                    headers: Vec::new(),
                    inputs: Vec::new(),
                    raw_query: None,
                },
                false,
            )
            .await?;
        if !response.status.is_success() {
            return Err(EngineError::UnexpectedStatus(response.status.as_u16()));
        }
        validate_nzb(&response.body)?;
        Ok(response.body)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn category_parameters_preserve_numeric_codes_and_map_aliases() {
        assert_eq!(
            newznab_category("Movies,5000,Books/Comics"),
            "2000,5000,7000"
        );
        assert_eq!(category_label("5030"), "TV");
        assert_eq!(category_label("unexpected"), "unexpected");
    }
    #[test]
    fn nzb_validation_enforces_root_and_files() {
        assert!(validate_nzb(b"<nzb><file><segments/></file></nzb>").is_ok());
        assert!(validate_nzb(b"<html><nzb/></html>").is_err());
        assert!(
            validate_nzb(
                b"<!DOCTYPE nzb [<!ENTITY x SYSTEM 'file:///etc/passwd'>]><nzb><file/></nzb>"
            )
            .is_err()
        );
    }
    #[test]
    fn parses_attributes_and_preserves_result_protocol() {
        let xml = r#"<rss xmlns:newznab="http://www.newznab.com/DTD/2010/feeds/attributes/"><channel><item><title>Sample &amp; Test</title><guid isPermaLink="false">abc</guid><link>https://example.org/get/abc</link><enclosure url="https://example.org/get/abc" length="123" type="application/x-nzb"/><newznab:attr name="files" value="5"/><newznab:attr name="category" value="5000"/></item></channel></rss>"#;
        let rows = parse_rss(xml, &Url::parse("https://example.org/").unwrap(), "a", "b").unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].protocol, ReleaseProtocol::Usenet);
        assert_eq!(rows[0].attributes.files, Some(5));
        assert_eq!(rows[0].size, Some(123));
    }
}
