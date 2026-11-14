use indexloom_core::SearchResult;
use serde_json::Value;
use time::{OffsetDateTime, PrimitiveDateTime, UtcOffset, format_description};
use uuid::Uuid;

use crate::{
    Error, HttpResponse, NativeAdapter, NativeIndexer, NativeQuery, NativeRelease, ReleaseMetadata,
    SearchPage,
};

pub(crate) fn status(response: &HttpResponse) -> Result<(), Error> {
    match response.status {
        200 => Ok(()),
        401 | 403 | 412 | 422 => Err(Error::Authentication),
        429 => Err(Error::RateLimited),
        300..=399 => Err(Error::Redirect),
        code => Err(Error::Http(code)),
    }
}

pub(crate) fn json(response: &HttpResponse) -> Result<Value, Error> {
    status(response)?;
    let mime = response
        .header("Content-Type")
        .unwrap_or("")
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    if mime != "application/json" && !(mime.starts_with("application/") && mime.ends_with("+json"))
    {
        return Err(Error::Parse("expected JSON content type"));
    }
    let body = response
        .body
        .strip_prefix(b"\xef\xbb\xbf")
        .unwrap_or(&response.body);
    let value: Value = serde_json::from_slice(body).map_err(|_| Error::Parse("invalid JSON"))?;
    if value.get("error").is_some() || value.get("errors").is_some() {
        return Err(Error::Api);
    }
    Ok(value)
}

pub(crate) fn text<'a>(value: &'a Value, key: &str) -> Result<&'a str, Error> {
    value
        .get(key)
        .and_then(Value::as_str)
        .filter(|v| !v.trim().is_empty() && v.len() <= 8192)
        .ok_or(Error::Parse("missing or invalid text field"))
}

pub(crate) fn optional_text(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .filter(|v| !v.trim().is_empty())
        .map(str::to_owned)
}

/// External IDs are strings in legacy APIs and numbers in newer deployments.
pub(crate) fn external_id(value: &Value, key: &str) -> Result<Option<String>, Error> {
    match value.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(v)) if v.len() <= 128 => {
            Ok((!v.trim().is_empty() && v != "0").then(|| v.clone()))
        }
        Some(Value::Number(v)) => v
            .as_u64()
            .map(|n| (n > 0).then(|| n.to_string()))
            .ok_or(Error::Parse("invalid external ID")),
        _ => Err(Error::Parse("invalid external ID")),
    }
}

pub(crate) fn number(value: &Value, key: &str) -> Result<Option<u64>, Error> {
    match value.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Number(v)) => v
            .as_u64()
            .map(Some)
            .ok_or(Error::Parse("invalid unsigned number")),
        Some(Value::String(v)) => v
            .parse::<u64>()
            .map(Some)
            .map_err(|_| Error::Parse("invalid numeric string")),
        _ => Err(Error::Parse("invalid numeric field")),
    }
}

pub(crate) fn required_number(value: &Value, key: &str) -> Result<u64, Error> {
    number(value, key)?.ok_or(Error::Parse("missing numeric field"))
}

pub(crate) fn count(value: &Value, key: &str) -> Result<Option<u32>, Error> {
    number(value, key)?
        .map(|n| u32::try_from(n).map_err(|_| Error::Parse("count overflow")))
        .transpose()
}

pub(crate) fn id(value: &Value) -> Result<String, Error> {
    match value.get("id") {
        Some(Value::String(v)) if !v.is_empty() && v.len() <= 128 => Ok(v.clone()),
        _ => required_number(value, "id").map(|n| n.to_string()),
    }
}

pub(crate) fn flag(value: &Value, key: &str) -> Result<bool, Error> {
    match value.get(key) {
        None | Some(Value::Null | Value::Bool(false)) => Ok(false),
        Some(Value::Bool(true)) => Ok(true),
        Some(Value::Number(v)) if v.as_u64() == Some(0) => Ok(false),
        Some(Value::Number(v)) if v.as_u64() == Some(1) => Ok(true),
        Some(Value::String(v)) if v == "0" || v == "1" => Ok(v == "1"),
        _ => Err(Error::Parse("invalid boolean flag")),
    }
}

pub(crate) fn factor(value: &Value, key: &str) -> Result<Option<f64>, Error> {
    match value.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => v
            .as_f64()
            .filter(|n| n.is_finite() && (0.0..=100.0).contains(n))
            .map(Some)
            .ok_or(Error::Parse("invalid volume factor")),
    }
}

pub(crate) fn date(value: &str, offset: UtcOffset) -> Result<OffsetDateTime, Error> {
    if let Ok(date) = OffsetDateTime::parse(value, &format_description::well_known::Rfc3339) {
        return Ok(date.to_offset(UtcOffset::UTC));
    }
    let format =
        format_description::parse_borrowed::<2>("[year]-[month]-[day] [hour]:[minute]:[second]")
            .map_err(|_| Error::Parse("date format"))?;
    PrimitiveDateTime::parse(value, &format)
        .map(|d| d.assume_offset(offset).to_offset(UtcOffset::UTC))
        .map_err(|_| Error::Parse("invalid timestamp"))
}

pub(crate) fn hash(value: &str) -> Option<String> {
    (value.len() == 40 && value.bytes().all(|v| v.is_ascii_hexdigit()))
        .then(|| value.to_ascii_lowercase())
}

pub(crate) fn release(
    adapter: &NativeAdapter,
    source_id: String,
    title: &str,
    details: &str,
    download: &str,
    metadata: ReleaseMetadata,
) -> Result<NativeRelease, Error> {
    let details = adapter.link(details)?;
    let download = adapter.link(download)?;
    let attributes = indexloom_core::ReleaseAttributes {
        files: metadata.files.and_then(|v| u32::try_from(v).ok()),
        grabs: metadata.grabs,
        imdb_id: metadata.imdb_id.clone(),
        tmdb_id: metadata.tmdb_id.clone(),
        tvdb_id: metadata.tvdb_id.clone(),
        download_volume_factor: metadata.download_volume_factor,
        upload_volume_factor: metadata.upload_volume_factor,
        languages: metadata.audio_languages.clone(),
        subtitles: metadata.subtitle_languages.clone(),
        ..Default::default()
    };
    Ok(NativeRelease {
        result: SearchResult {
            id: Uuid::now_v7(),
            indexer_instance_id: adapter.instance_id().to_owned(),
            definition_id: adapter.site_id().to_owned(),
            source_id: Some(source_id),
            source_guid: Some(details.to_string()),
            info_hash: None,
            title: title.trim().to_owned(),
            details_url: Some(details.to_string()),
            download_url: None,
            magnet_url: None,
            category: None,
            size: None,
            seeders: None,
            leechers: None,
            published_at: None,
            protocol: indexloom_core::ReleaseProtocol::Torrent,
            attributes,
        },
        metadata,
        download,
        owner: adapter.session_id,
    })
}

pub(crate) fn rows(value: &Value) -> Result<&Vec<Value>, Error> {
    value
        .get("data")
        .and_then(Value::as_array)
        .ok_or(Error::Parse("missing data array"))
}

/// Validate a supplied continuation URL, then discard it. Only a fresh request
/// built from the original query will ever be sent, never an upstream next URL.
pub(crate) fn next_page(
    adapter: &NativeAdapter,
    value: Option<&Value>,
    endpoint: &str,
    query: &NativeQuery,
) -> Result<Option<u32>, Error> {
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(v)) if v.is_empty() => Ok(None),
        Some(Value::String(v)) => {
            let url = adapter.link(v)?;
            let expected = adapter.endpoint(endpoint)?;
            if url.path() != expected.path() {
                return Err(Error::UnsafeUrl);
            }
            let pages: Vec<_> = url
                .query_pairs()
                .filter(|(k, _)| k == "page")
                .map(|(_, v)| v.parse::<u32>())
                .collect();
            if pages.len() != 1 || pages[0].as_ref().ok() != Some(&(query.page + 1)) {
                return Err(Error::Parse("invalid continuation page"));
            }
            Ok((query.page < 10_000).then_some(query.page + 1))
        }
        _ => Err(Error::Parse("invalid continuation link")),
    }
}

pub(crate) fn finish(
    mut releases: Vec<NativeRelease>,
    query: &NativeQuery,
    next_page: Option<u32>,
) -> SearchPage {
    releases.sort_by_key(|r| std::cmp::Reverse(r.result.published_at));
    let truncated = releases.len() > query.limit as usize;
    releases.truncate(query.limit as usize);
    SearchPage {
        releases,
        next_page,
        truncated,
    }
}

pub(crate) fn imdb(query: &NativeQuery) -> Option<String> {
    query
        .imdb_id
        .as_ref()
        .map(|id| format!("tt{}", id.strip_prefix("tt").unwrap_or(id)))
}

pub(crate) fn episode(query: &NativeQuery) -> String {
    match (query.season, query.episode) {
        (Some(s), Some(e)) if s > 0 => format!("S{s:02}E{e:02}"),
        (_, Some(e)) => format!("E{e:02}"),
        (Some(s), None) if s > 0 => format!("S{s:02}"),
        _ => String::new(),
    }
}
