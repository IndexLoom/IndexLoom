use time::UtcOffset;

use crate::{
    Error, HttpRequest, Method, NativeAdapter, NativeQuery, ReleaseMetadata, SearchPage, Secret,
    Unit3dApi, adapter::Session, parse,
};
use serde_json::Value;

const ENDPOINT: &str = "api/torrents/filter";

pub(crate) async fn search(
    adapter: &NativeAdapter,
    session: &mut Session,
    q: &NativeQuery,
    token: &Secret,
    api: Unit3dApi,
) -> Result<SearchPage, Error> {
    let request = build_request(adapter, q, token, api)?;
    let response = adapter.send(session, request).await?;
    let value = parse::json(&response)?;
    let mut releases = Vec::new();
    for row in parse::rows(&value)? {
        let attrs = row
            .get("attributes")
            .ok_or(Error::Parse("missing UNIT3D attributes"))?;
        let featured = parse::flag(attrs, "featured")?;
        let factor = if featured {
            0.0
        } else {
            freeleech_factor(attrs, api)?
        };
        let free = factor == 0.0;
        if q.freeleech_only && !free {
            continue;
        }
        let id = parse::id(row)?;
        let meta = ReleaseMetadata {
            files: parse::number(attrs, "num_file")?,
            grabs: parse::number(attrs, "times_completed")?,
            imdb_id: parse::external_id(attrs, "imdb_id")?,
            tmdb_id: parse::external_id(attrs, "tmdb_id")?,
            tvdb_id: parse::external_id(attrs, "tvdb_id")?,
            internal: parse::flag(attrs, "internal")?,
            download_volume_factor: Some(factor),
            upload_volume_factor: Some(if featured || parse::flag(attrs, "double_upload")? {
                2.0
            } else {
                1.0
            }),
            ..ReleaseMetadata::default()
        };
        let mut r = parse::release(
            adapter,
            id.clone(),
            parse::text(attrs, "name")?,
            parse::text(attrs, "details_link")?,
            parse::text(attrs, "download_link")?,
            meta,
        )?;
        // Some deployments use a numeric resource ID; it is not an info hash.
        r.result.info_hash = parse::hash(&id);
        r.result.size = Some(parse::required_number(attrs, "size")?);
        r.result.seeders = parse::count(attrs, "seeders")?;
        r.result.leechers = parse::count(attrs, "leechers")?;
        r.result.category = parse::optional_text(attrs, "category").or_else(|| {
            attrs
                .get("category_id")
                .and_then(Value::as_u64)
                .map(|n| n.to_string())
        });
        r.result.published_at = Some(parse::date(
            parse::text(attrs, "created_at")?,
            UtcOffset::UTC,
        )?);
        releases.push(r);
    }
    let next = parse::next_page(
        adapter,
        value.get("links").and_then(|v| v.get("next")),
        ENDPOINT,
        q,
    )?;
    Ok(parse::finish(releases, q, next))
}

fn build_request(
    adapter: &NativeAdapter,
    q: &NativeQuery,
    token: &Secret,
    api: Unit3dApi,
) -> Result<HttpRequest, Error> {
    let mut url = adapter.endpoint(ENDPOINT)?;
    {
        let mut params = url.query_pairs_mut();
        if api == Unit3dApi::Legacy {
            params.append_pair("api_token", token.expose());
        }
        params.append_pair("page", &q.page.to_string());
        params.append_pair("perPage", &q.limit.to_string());
        let term = if api == Unit3dApi::Legacy {
            format!("{} {}", q.term.trim(), parse::episode(q))
                .trim()
                .to_owned()
        } else {
            if let Some(s) = q.season {
                params.append_pair("seasonNumber", &s.to_string());
            }
            if let Some(e) = q.episode {
                params.append_pair("episodeNumber", &e.to_string());
            }
            params
                .append_pair("sortField", "created_at")
                .append_pair("sortDirection", "desc");
            q.term.trim().replace('.', " ")
        };
        if !term.is_empty() {
            params.append_pair("name", &term);
        }
        for category in &q.native_categories {
            params.append_pair("categories[]", &category.to_string());
        }
        if let Some(imdb) = parse::imdb(q) {
            // Both spellings are used across deployed UNIT3D versions.
            let id = imdb.trim_start_matches("tt");
            if api == Unit3dApi::Legacy {
                params.append_pair("imdb", id);
            }
            params.append_pair("imdbId", id);
        }
        if q.freeleech_only && api == Unit3dApi::Modern {
            params.append_pair("free[]", "100");
        }
    }
    let mut request = adapter.request(Method::Get, url, false);
    if api == Unit3dApi::Modern {
        request.headers.insert(
            "Authorization".into(),
            Secret::new(format!("Bearer {}", token.expose())),
        );
    }
    Ok(request)
}

fn freeleech_factor(attrs: &Value, api: Unit3dApi) -> Result<f64, Error> {
    if api == Unit3dApi::Legacy {
        return Ok(if parse::flag(attrs, "freeleech")? {
            0.0
        } else {
            1.0
        });
    }
    let percent = match attrs.get("freeleech") {
        Some(Value::String(v)) => v.trim_end_matches('%').parse::<u32>().ok(),
        Some(Value::Number(v)) => v.as_u64().and_then(|n| u32::try_from(n).ok()),
        _ => None,
    }
    .filter(|n| [0, 25, 50, 75, 100].contains(n))
    .ok_or(Error::Parse("invalid freeleech percentage"))?;
    Ok(1.0 - f64::from(percent) / 100.0)
}
