use serde_json::Value;
use time::UtcOffset;
use zeroize::Zeroizing;

use crate::{
    Credentials, Error, HttpRequest, HttpResponse, Method, NativeAdapter, NativeQuery,
    ReleaseMetadata, SearchPage, Secret, adapter::Session, parse,
};

const ENDPOINT: &str = "api/v1/jackett/torrents";

async fn login(adapter: &NativeAdapter, session: &mut Session) -> Result<(), Error> {
    let Credentials::Avistaz {
        username,
        password,
        pid,
    } = &adapter.credentials
    else {
        return Err(Error::Configuration("AvistaZ credentials required"));
    };
    session.token = None;
    let mut request = adapter.request(
        Method::Post,
        adapter.endpoint("api/v1/jackett/auth")?,
        false,
    );
    request.headers.insert(
        "Content-Type".into(),
        Secret::new("application/x-www-form-urlencoded"),
    );
    let form = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("username", username.expose())
        .append_pair("password", password.expose())
        .append_pair("pid", pid.expose())
        .finish();
    request.body = Zeroizing::new(form.into_bytes());
    let response = adapter.send(session, request).await?;
    let value = parse::json(&response)?;
    let token = parse::text(&value, "token").map_err(|_| Error::Authentication)?;
    if token.chars().any(|c| c.is_control() || c.is_whitespace()) || token.len() > 4096 {
        return Err(Error::Authentication);
    }
    session.token = Some(Secret::new(token));
    Ok(())
}

pub(crate) async fn authorized(
    adapter: &NativeAdapter,
    session: &mut Session,
    mut request: HttpRequest,
) -> Result<HttpResponse, Error> {
    if session.token.is_none() {
        login(adapter, session).await?;
    }
    for refresh in 0..=1 {
        let token = session.token.as_ref().ok_or(Error::Authentication)?;
        request.headers.insert(
            "Authorization".into(),
            Secret::new(format!("Bearer {}", token.expose())),
        );
        let response = adapter.send(session, request.clone()).await?;
        if matches!(response.status, 401 | 412) {
            session.token = None;
            if refresh == 0 {
                login(adapter, session).await?;
                continue;
            }
            return Err(Error::Authentication);
        }
        return Ok(response);
    }
    unreachable!("bounded refresh returns on last iteration")
}

fn languages(row: &Value, key: &str) -> Result<Vec<String>, Error> {
    match row.get(key) {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(Value::Array(values)) if values.len() <= 256 => values
            .iter()
            .map(|v| parse::text(v, "language").map(str::to_owned))
            .collect(),
        _ => Err(Error::Parse("invalid language list")),
    }
}

pub(crate) async fn search(
    adapter: &NativeAdapter,
    session: &mut Session,
    q: &NativeQuery,
) -> Result<SearchPage, Error> {
    let mut url = adapter.endpoint(ENDPOINT)?;
    {
        let mut params = url.query_pairs_mut();
        let mut cats = q.native_categories.clone();
        cats.sort_unstable();
        cats.dedup();
        params.append_pair("in", "1");
        params.append_pair(
            "type",
            &if cats.len() == 1 {
                cats[0].to_string()
            } else {
                "0".to_owned()
            },
        );
        params
            .append_pair("limit", &q.limit.to_string())
            .append_pair("page", &q.page.to_string());
        let episode = parse::episode(q);
        if let Some(imdb) = parse::imdb(q) {
            params
                .append_pair("imdb", &imdb)
                .append_pair("search", &episode);
        } else {
            params.append_pair("search", format!("{} {}", q.term.trim(), episode).trim());
        }
        if q.freeleech_only {
            params.append_pair("discount[]", "1");
        }
    }
    let response = authorized(adapter, session, adapter.request(Method::Get, url, false)).await?;
    if response.status == 404 {
        return Ok(SearchPage::default());
    }
    let value = parse::json(&response)?;
    let mut releases = Vec::new();
    for row in parse::rows(&value)? {
        let download_factor = parse::factor(row, "download_multiply")?;
        if q.freeleech_only && download_factor != Some(0.0) {
            continue;
        }
        let category = parse::text(row, "type")?;
        // type=0 returns all categories; retain only explicitly requested ones.
        let category_id = match category.to_ascii_lowercase().as_str() {
            "movie" => 1,
            "tv-show" => 2,
            _ => continue,
        };
        if !q.native_categories.is_empty() && !q.native_categories.contains(&category_id) {
            continue;
        }
        let ids = row.get("movie_tv").unwrap_or(&Value::Null);
        let meta = ReleaseMetadata {
            files: parse::number(row, "file_count")?,
            grabs: parse::number(row, "completed")?,
            imdb_id: parse::external_id(ids, "imdb")?,
            tmdb_id: parse::external_id(ids, "tmdb")?,
            tvdb_id: parse::external_id(ids, "tvdb")?,
            download_volume_factor: download_factor,
            upload_volume_factor: parse::factor(row, "upload_multiply")?,
            audio_languages: languages(row, "audio")?,
            subtitle_languages: languages(row, "subtitle")?,
            ..ReleaseMetadata::default()
        };
        let mut r = parse::release(
            adapter,
            parse::id(row)?,
            parse::text(row, "file_name")?,
            parse::text(row, "url")?,
            parse::text(row, "download")?,
            meta,
        )?;
        r.result.info_hash = parse::optional_text(row, "info_hash").and_then(|h| parse::hash(&h));
        r.result.size = parse::number(row, "file_size")?;
        r.result.seeders = parse::count(row, "seed")?;
        r.result.leechers = parse::count(row, "leech")?;
        r.result.category = Some(category.to_owned());
        r.result.published_at = Some(parse::date(
            parse::text(row, "created_at_iso")?,
            UtcOffset::UTC,
        )?);
        releases.push(r);
    }
    let next = parse::next_page(adapter, value.get("next_page_url"), ENDPOINT, q)?;
    Ok(parse::finish(releases, q, next))
}
