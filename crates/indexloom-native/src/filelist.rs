use base64::{Engine as _, engine::general_purpose::STANDARD};
use time::UtcOffset;
use zeroize::Zeroizing;

use crate::{
    Error, Method, NativeAdapter, NativeQuery, ReleaseMetadata, SearchPage, Secret,
    adapter::Session, parse,
};

pub(crate) async fn search(
    adapter: &NativeAdapter,
    session: &mut Session,
    q: &NativeQuery,
    username: &Secret,
    passkey: &Secret,
) -> Result<SearchPage, Error> {
    let mut url = adapter.endpoint("api.php")?;
    {
        let mut params = url.query_pairs_mut();
        let imdb = parse::imdb(q);
        if !q.term.trim().is_empty() || imdb.is_some() {
            params.append_pair("action", "search-torrents");
            params.append_pair("type", if imdb.is_some() { "imdb" } else { "name" });
            params.append_pair("query", imdb.as_deref().unwrap_or(q.term.trim()));
            if let Some(s) = q.season {
                params.append_pair("season", &s.to_string());
            }
            if let Some(e) = q.episode {
                params.append_pair("episode", &e.to_string());
            }
        } else {
            params.append_pair("action", "latest-torrents");
        }
        if !q.native_categories.is_empty() {
            params.append_pair(
                "category",
                &q.native_categories
                    .iter()
                    .map(u32::to_string)
                    .collect::<Vec<_>>()
                    .join(","),
            );
        }
        if q.freeleech_only {
            params.append_pair("freeleech", "1");
        }
    }
    let mut request = adapter.request(Method::Get, url, false);
    let basic = Zeroizing::new(format!("{}:{}", username.expose(), passkey.expose()));
    request.headers.insert(
        "Authorization".into(),
        Secret::new(format!("Basic {}", STANDARD.encode(basic.as_bytes()))),
    );
    let response = adapter.send(session, request).await?;
    let value = parse::json(&response)?;
    let rows = value
        .as_array()
        .ok_or(Error::Parse("expected FileList array"))?;
    let mut releases = Vec::new();
    for row in rows {
        let free = parse::flag(row, "freeleech")?;
        if q.freeleech_only && !free {
            continue;
        }
        let id = parse::required_number(row, "id")?.to_string();
        let mut details = adapter.endpoint("details.php")?;
        details.query_pairs_mut().append_pair("id", &id);
        // Build with the configured origin/passkey. Older API download_link
        // values may still point at the tracker's retired hostname.
        let mut download = adapter.endpoint("download.php")?;
        download
            .query_pairs_mut()
            .append_pair("id", &id)
            .append_pair("passkey", passkey.expose());
        let meta = ReleaseMetadata {
            files: parse::number(row, "files")?,
            grabs: parse::number(row, "times_completed")?,
            imdb_id: parse::external_id(row, "imdb")?,
            internal: parse::flag(row, "internal")?,
            download_volume_factor: Some(if free { 0.0 } else { 1.0 }),
            upload_volume_factor: Some(if parse::flag(row, "doubleup")? {
                2.0
            } else {
                1.0
            }),
            ..ReleaseMetadata::default()
        };
        let mut r = parse::release(
            adapter,
            id,
            parse::text(row, "name")?,
            details.as_str(),
            download.as_str(),
            meta,
        )?;
        r.result.size = Some(parse::required_number(row, "size")?);
        r.result.seeders = parse::count(row, "seeders")?;
        r.result.leechers = parse::count(row, "leechers")?;
        r.result.category = parse::optional_text(row, "category");
        // API's timezone-less upload_date is treated as fixed +02:00 by both
        // audited implementations, including summer dates.
        r.result.published_at = Some(parse::date(
            parse::text(row, "upload_date")?,
            UtcOffset::from_hms(2, 0, 0).map_err(|_| Error::Parse("date offset"))?,
        )?);
        releases.push(r);
    }
    Ok(parse::finish(releases, q, None))
}
