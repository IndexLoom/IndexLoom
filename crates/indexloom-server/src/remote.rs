//! External mode is intentionally a redirect-only launcher: no proxying of
//! credentials or tracker traffic through the device running this binary.
use axum::{
    Router,
    extract::{State, Uri},
    http::StatusCode,
    response::{IntoResponse, Redirect, Response},
};
use url::Url;

/// Redirect all methods with HTTP 307; preserving methods is important for
/// clients that start from a local address. Browsers should use the remote
/// HTTPS origin directly after the first navigation.
pub fn router(remote: Url) -> Router {
    Router::new().fallback(redirect).with_state(remote)
}

async fn redirect(State(remote): State<Url>, uri: Uri) -> Response {
    match target_url(&remote, &uri) {
        Some(url) => {
            let mut response = Redirect::temporary(url.as_str()).into_response();
            response.headers_mut().insert(
                axum::http::header::CACHE_CONTROL,
                axum::http::HeaderValue::from_static("no-store"),
            );
            response
        }
        None => (StatusCode::BAD_REQUEST, "unsafe request target").into_response(),
    }
}

fn target_url(remote: &Url, uri: &Uri) -> Option<Url> {
    let path = uri.path();
    // A URI beginning // can be interpreted as an authority by some clients,
    // and backslashes have inconsistent URL parser semantics.
    if !path.starts_with('/') || path.starts_with("//") || path.contains('\\') {
        return None;
    }
    let mut url = remote.clone();
    url.set_path(path);
    url.set_query(uri.query());
    if url.origin() != remote.origin() { return None; }
    Some(url)
}

#[cfg(test)]
mod tests {
    use super::target_url;
    use axum::http::Uri;
    use url::Url;

    #[test]
    fn preserves_query_but_never_changes_remote_origin() {
        let site = Url::parse("https://indexloom.example/").unwrap();
        let request = Uri::from_static("/api/v1/search?q=%D1%82%D0%B5%D1%81%D1%82");
        let target = target_url(&site, &request).unwrap();
        assert_eq!(target.origin(), site.origin());
        assert_eq!(target.path(), "/api/v1/search");
        assert_eq!(target.query(), Some("q=%D1%82%D0%B5%D1%81%D1%82"));
        assert!(target_url(&site, &Uri::from_static("//attacker.example/")).is_none());
    }
}
