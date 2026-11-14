use super::*;

fn url(value: &str) -> Url {
    Url::parse(value).unwrap()
}
fn set(jar: &SessionJar, target: &str, values: &[&str]) {
    let headers = values
        .iter()
        .map(|value| HeaderValue::from_str(value).unwrap())
        .collect::<Vec<_>>();
    jar.set_cookies(&mut headers.iter(), &url(target));
}
fn get(jar: &SessionJar, target: &str) -> String {
    jar.cookies(&url(target))
        .map(|header| header.to_str().unwrap().to_owned())
        .unwrap_or_default()
}

#[test]
fn scopes_host_domain_path_secure_and_isolates_jars_after_restart() {
    let jar = SessionJar::default();
    set(
        &jar,
        "https://tracker.example.com/login",
        &[
            "host=private; Path=/; HttpOnly; SameSite=Strict",
            "domain=shared; Domain=example.com; Path=/",
            "path=private; Path=/account; Secure",
            "foreign=bad; Domain=attacker.test; Path=/",
            "suffix=bad; Domain=com; Path=/",
        ],
    );
    let restored = SessionJar::default();
    restored.restore(&jar.snapshot().unwrap().unwrap()).unwrap();
    let cookie_set = |jar: &SessionJar| {
        let value = get(jar, "https://tracker.example.com/account");
        value
            .split("; ")
            .map(str::to_owned)
            .collect::<std::collections::BTreeSet<_>>()
    };
    assert_eq!(cookie_set(&jar), cookie_set(&restored));
    assert!(get(&restored, "https://tracker.example.com/account").starts_with("path=private;"));
    assert_eq!(
        get(&restored, "https://other.example.com/"),
        "domain=shared"
    );
    assert!(!get(&restored, "http://tracker.example.com/account").contains("path="));
    assert!(!get(&restored, "https://tracker.example.com/accounting").contains("path="));
    assert!(get(&restored, "https://attacker.test/").is_empty());
    assert!(get(&restored, "https://evil-example.com/").is_empty());
    assert!(!get(&restored, "https://tracker.example.com/").contains("bad"));
    assert!(get(&SessionJar::default(), "https://tracker.example.com/").is_empty());
    assert!(!format!("{jar:?}").contains("private"));
    assert!(!format!("{:?}", jar.snapshot().unwrap()).contains("private"));
    assert!(
        restored
            .cookies(&url("https://tracker.example.com/"))
            .unwrap()
            .is_sensitive()
    );
}

#[test]
fn expires_absolute_deadlines_session_fallback_and_max_age_precedence() {
    let jar = SessionJar::default();
    let before = OffsetDateTime::now_utc();
    set(
        &jar,
        "https://tracker.test/login/page",
        &[
            "session=secret",
            "past=bad; Expires=Thu, 01 Jan 1970 00:00:00 GMT",
            "max=kept; Max-Age=60; Expires=Thu, 01 Jan 1970 00:00:00 GMT",
        ],
    );
    assert!(!get(&jar, "https://tracker.test/login/page").contains("past="));
    assert!(!get(&jar, "https://tracker.test/other").contains("session="));
    let first = jar.snapshot().unwrap().unwrap();
    let encoded = first.cookies_json.clone();
    assert!(first.expires_at >= (before + MAX_COOKIE_AGE).unix_timestamp());
    let restored = SessionJar::default();
    restored.restore(&first).unwrap();
    let cookies: Vec<Cookie<'static>> = serde_json::from_str(&encoded).unwrap();
    let restored_cookies: Vec<Cookie<'static>> =
        serde_json::from_str(&restored.snapshot().unwrap().unwrap().cookies_json).unwrap();
    for cookie in cookies {
        let copy = restored_cookies
            .iter()
            .find(|other| other.name() == cookie.name())
            .unwrap();
        // JSON has second precision; there is no fresh Max-Age calculation.
        assert_eq!(cookie.expires, copy.expires);
    }
    let expired = SessionJar::default();
    expired
        .restore(&CookieSnapshot {
            cookies_json: encoded,
            expires_at: before.unix_timestamp() - 1,
        })
        .unwrap();
    assert!(get(&expired, "https://tracker.test/login/page").is_empty());
    assert!(expired.snapshot().unwrap().is_none());
}

#[test]
fn cookie_deletion_and_rotation_survive_restart() {
    let jar = SessionJar::default();
    set(
        &jar,
        "https://tracker.test/",
        &["sid=old; Path=/", "csrf=old; Path=/"],
    );
    set(
        &jar,
        "https://tracker.test/",
        &["sid=new; Path=/", "csrf=deleted; Path=/; Max-Age=0"],
    );
    let restored = SessionJar::default();
    restored.restore(&jar.snapshot().unwrap().unwrap()).unwrap();
    assert_eq!(get(&restored, "https://tracker.test/"), "sid=new");
    set(
        &restored,
        "https://tracker.test/",
        &["sid=deleted; Path=/; Max-Age=-1"],
    );
    assert!(restored.snapshot().unwrap().is_none());
}

#[test]
fn secure_prefixes_downgrade_and_public_suffix_rejection() {
    let jar = SessionJar::default();
    set(
        &jar,
        "https://tracker.co.uk/",
        &[
            "__Host-session=good; Secure; Path=/",
            "__Host-domain=bad; Secure; Path=/; Domain=tracker.co.uk",
            "__Secure-session=bad; Path=/",
            "suffix=bad; Domain=co.uk",
        ],
    );
    set(
        &jar,
        "http://tracker.co.uk/",
        &[
            "__Host-session=bad; Secure; Path=/",
            "__Host-session=bad; Path=/",
        ],
    );
    assert_eq!(get(&jar, "https://tracker.co.uk/"), "__Host-session=good");
    set(
        &jar,
        "https://tracker.co.uk/",
        &["sid=secure; Secure; Path=/"],
    );
    set(&jar, "http://tracker.co.uk/", &["sid=insecure; Path=/"]);
    assert!(get(&jar, "https://tracker.co.uk/").contains("sid=secure"));
    // Multiple insertions/pruning must retain the suffix policy.
    set(&jar, "https://tracker.co.uk/", &["again=bad; Domain=co.uk"]);
    assert!(get(&jar, "https://other.co.uk/").is_empty());
}

#[test]
fn manual_cookie_seed_is_host_only_and_never_downgrades() {
    let jar = SessionJar::default();
    jar.seed("sid=fixture; csrf=fixture", &url("https://tracker.test/"));
    assert!(get(&jar, "https://tracker.test/").contains("sid=fixture"));
    assert!(get(&jar, "http://tracker.test/").is_empty());
    assert!(get(&jar, "https://sub.tracker.test/").is_empty());
}

#[test]
fn bounded_jar_still_accepts_updates_and_deletions() {
    let jar = SessionJar::default();
    for index in 0..MAX_COOKIES + 10 {
        set(
            &jar,
            "https://tracker.test/",
            &[&format!("key{index}=value; Path=/")],
        );
    }
    assert_eq!(jar.0.lock().unwrap().iter_unexpired().count(), MAX_COOKIES);
    set(
        &jar,
        "https://tracker.test/",
        &["key0=rotated; Path=/", "key1=deleted; Path=/; Max-Age=0"],
    );
    assert!(get(&jar, "https://tracker.test/").contains("key0=rotated"));
    assert!(!get(&jar, "https://tracker.test/").contains("key1="));
    assert!(
        jar.restore(&CookieSnapshot {
            cookies_json: "[invalid-cookie-secret]".into(),
            expires_at: 0
        })
        .is_err()
    );
}

#[test]
fn browser_bridge_filters_scope_and_retains_expiration() {
    let jar = SessionJar::default();
    let expires = OffsetDateTime::now_utc().unix_timestamp() + 60;
    let cookie = crate::BrowserCookie {
        name: "solver".into(),
        value: "fixture".into(),
        domain: Some("tracker.test".into()),
        path: Some("/search".into()),
        secure: true,
        http_only: true,
        expires: Some(time::Duration::seconds(expires).as_seconds_f64()),
    };
    jar.receive_browser_cookies(&[cookie], &url("https://tracker.test/"));
    assert_eq!(
        jar.browser_cookies(&url("https://tracker.test/search"))
            .len(),
        1
    );
    assert!(
        jar.browser_cookies(&url("https://other.test/search"))
            .is_empty()
    );
    assert!(
        jar.browser_cookies(&url("http://tracker.test/search"))
            .is_empty()
    );
    assert!(jar.snapshot().unwrap().unwrap().expires_at <= expires);
    assert!(
        jar.browser_cookies(&url("https://sub.tracker.test/search"))
            .is_empty()
    );
    assert!(
        jar.browser_cookies(&url("https://tracker.test/search"))[0]
            .domain
            .is_none()
    );
    let cookie = crate::BrowserCookie {
        name: "domain".into(),
        value: "fixture-domain".into(),
        domain: Some(".tracker.test".into()),
        path: Some("/".into()),
        secure: true,
        http_only: false,
        expires: Some(time::Duration::seconds(expires).as_seconds_f64()),
    };
    jar.receive_browser_cookies(&[cookie], &url("https://tracker.test/"));
    assert_eq!(
        jar.browser_cookies(&url("https://sub.tracker.test/"))[0]
            .domain
            .as_deref(),
        Some(".tracker.test")
    );
}

#[test]
fn browser_bridge_rejects_attribute_injection_and_foreign_hosts() {
    let jar = SessionJar::default();
    let fixture = crate::BrowserCookie {
        name: "sid".into(),
        value: "fixture".into(),
        domain: Some("tracker.example.com".into()),
        path: Some("/".into()),
        secure: true,
        http_only: true,
        expires: None,
    };
    let mut invalid = Vec::new();
    for (name, value) in [
        ("sid; Domain=other.example.com", "fixture"),
        ("sid", "fixture; Domain=other.example.com"),
        ("sid", "fixture\r\nSet-Cookie: injected=bad"),
    ] {
        invalid.push(crate::BrowserCookie {
            name: name.into(),
            value: value.into(),
            ..fixture.clone()
        });
    }
    for domain in [
        "other.example.com",
        ".other.example.com",
        ".com",
        ".tracker.example.com; Path=/",
    ] {
        invalid.push(crate::BrowserCookie {
            domain: Some(domain.into()),
            ..fixture.clone()
        });
    }
    invalid.push(crate::BrowserCookie {
        path: Some("/; Domain=other.example.com".into()),
        ..fixture.clone()
    });
    for (index, cookie) in invalid.into_iter().enumerate() {
        jar.receive_browser_cookies(&[cookie], &url("https://tracker.example.com/"));
        assert!(jar.snapshot().unwrap().is_none(), "invalid case {index}");
    }
    jar.receive_browser_cookies(&[fixture], &url("https://tracker.example.com/"));
    assert_eq!(get(&jar, "https://tracker.example.com/"), "sid=fixture");
    assert!(get(&jar, "https://other.example.com/").is_empty());
}
