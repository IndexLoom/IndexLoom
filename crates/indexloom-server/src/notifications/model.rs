use crate::ApiError;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use url::{Host, Url};

pub const EVENTS: &[(&str, &str, &str)] = &[
    (
        "system.startup",
        "IndexLoom started",
        "The notification worker is ready.",
    ),
    (
        "auth.login",
        "Account sign-in",
        "An account signed in to IndexLoom.",
    ),
    (
        "automation.failed",
        "Automation failed",
        "A scheduled job failed. Review automation diagnostics.",
    ),
    (
        "automation.completed",
        "Automation completed",
        "A scheduled job completed successfully.",
    ),
    (
        "indexer.test.failed",
        "Indexer test failed",
        "An indexer test failed. Review indexer diagnostics.",
    ),
    (
        "indexer.test.passed",
        "Indexer test passed",
        "An indexer test completed successfully.",
    ),
];
pub fn known_event(event: &str) -> bool {
    EVENTS.iter().any(|e| e.0 == event)
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Config {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_base: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chat_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<i64>,
    #[serde(default)]
    pub disable_notification: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub topic: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub priority: Option<i32>,
    #[serde(default)]
    pub allow_insecure_local: bool,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ChannelInput {
    pub name: String,
    pub provider: String,
    pub enabled: bool,
    pub config: Config,
    pub events: Vec<String>,
    /// Missing = preserve, null = clear, string = replace. Never serialized.
    #[serde(default)]
    pub secrets: BTreeMap<String, Option<String>>,
    #[serde(default)]
    pub revision: i64,
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChannelView {
    pub id: String,
    pub name: String,
    pub provider: String,
    pub enabled: bool,
    pub config: Config,
    pub events: Vec<String>,
    pub stored_secrets: Vec<String>,
    pub revision: i64,
}
pub fn secret_fields(provider: &str) -> Option<&'static [&'static str]> {
    Some(match provider {
        "telegram" | "gotify" | "ntfy" => &["token"],
        "discord" | "slack" => &["url"],
        "webhook" => &["url", "bearer"],
        "apprise" => &["url", "key"],
        "pushover" => &["token", "user"],
        _ => return None,
    })
}
pub fn view(c: &indexloom_storage::notifications::Channel) -> Result<ChannelView, ApiError> {
    Ok(ChannelView {
        id: c.id.clone(),
        name: c.name.clone(),
        provider: c.provider.clone(),
        enabled: c.enabled,
        config: serde_json::from_str(&c.config)
            .map_err(|_| ApiError::internal("invalid stored notification configuration"))?,
        events: serde_json::from_str(&c.events)
            .map_err(|_| ApiError::internal("invalid stored notification events"))?,
        stored_secrets: c.secrets.keys().cloned().collect(),
        revision: c.revision,
    })
}
/// HTTP is an explicit exception restricted to literal private/loopback IPs or localhost.
/// Public bases cannot contain userinfo, queries or fragments. Credential URLs are encrypted.
pub fn validate_url(value: &str, local: bool, base: bool) -> Result<Url, ApiError> {
    let u = Url::parse(value).map_err(|_| ApiError::bad_request("invalid notification URL"))?;
    let local_host = match u.host() {
        Some(Host::Domain("localhost")) => true,
        Some(Host::Ipv4(ip)) => ip.is_loopback() || ip.is_private(),
        Some(Host::Ipv6(ip)) => ip.is_loopback() || ip.is_unique_local(),
        _ => false,
    };
    if value.len() > 2048
        || u.host().is_none()
        || !u.username().is_empty()
        || u.password().is_some()
        || u.fragment().is_some()
        || (base && u.query().is_some())
        || !(u.scheme() == "https" || (u.scheme() == "http" && local && local_host))
    {
        return Err(ApiError::bad_request(
            "notification URL requires HTTPS; explicit local HTTP allows only localhost or private IPs without credentials",
        ));
    }
    Ok(u)
}
fn segment(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 200
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_-.".contains(&b))
        && s != "."
        && s != ".."
}
pub fn validate(
    input: &ChannelInput,
    secrets: &BTreeMap<String, String>,
    sending: bool,
) -> Result<(), ApiError> {
    let allowed = secret_fields(&input.provider)
        .ok_or_else(|| ApiError::bad_request("unsupported notification provider"))?;
    if input.name.trim().is_empty()
        || input.name.len() > 120
        || input.name.chars().any(char::is_control)
        || input.events.len() > EVENTS.len()
        || input.events.iter().any(|e| !known_event(e))
        || input
            .events
            .iter()
            .collect::<std::collections::BTreeSet<_>>()
            .len()
            != input.events.len()
        || input.secrets.keys().any(|k| !allowed.contains(&k.as_str()))
        || secrets.iter().any(|(k, v)| {
            !allowed.contains(&k.as_str())
                || v.is_empty()
                || v.len() > 2048
                || v.chars().any(char::is_control)
                || (k != "url" && (!v.is_ascii() || v.bytes().any(|b| b.is_ascii_whitespace())))
        })
    {
        return Err(ApiError::bad_request(
            "invalid notification name, events or credentials",
        ));
    }
    let c = &input.config;
    if c.chat_id.as_ref().is_some_and(|s| {
        s.is_empty()
            || s.len() > 128
            || !s
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"@_-".contains(&b))
    }) || c.thread_id.is_some_and(|n| n <= 0)
        || c.topic.as_ref().is_some_and(|t| !segment(t))
    {
        return Err(ApiError::bad_request("invalid chat, thread or topic"));
    }
    if (input.provider != "telegram"
        && (c.chat_id.is_some() || c.thread_id.is_some() || c.disable_notification))
        || (input.provider != "ntfy" && c.topic.is_some())
        || (!matches!(input.provider.as_str(), "telegram" | "ntfy" | "gotify")
            && c.api_base.is_some())
        || (!matches!(input.provider.as_str(), "ntfy" | "gotify" | "pushover")
            && c.priority.is_some())
    {
        return Err(ApiError::bad_request(
            "configuration fields do not belong to this provider",
        ));
    }
    if let Some(base) = &c.api_base {
        validate_url(base, c.allow_insecure_local, true)?;
    }
    if let Some(url) = secrets.get("url") {
        validate_url(url, c.allow_insecure_local, input.provider == "apprise")?;
    }
    if let Some(key) = secrets.get("key")
        && !segment(key)
    {
        return Err(ApiError::bad_request("invalid Apprise key"));
    }
    if let Some(token) = secrets.get("token")
        && input.provider == "telegram"
        && (!token
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_:-".contains(&b))
            || !token.contains(':'))
    {
        return Err(ApiError::bad_request("invalid Telegram token format"));
    }
    let range = match input.provider.as_str() {
        "ntfy" => 1..=5,
        "pushover" => -2..=1,
        _ => 0..=10,
    };
    if c.priority.is_some_and(|n| !range.contains(&n)) {
        return Err(ApiError::bad_request("invalid notification priority"));
    }
    if sending || input.enabled {
        let required: &[&str] = match input.provider.as_str() {
            "ntfy" => &[],
            "webhook" => &["url"],
            _ => allowed,
        };
        if required.iter().any(|k| !secrets.contains_key(*k))
            || (input.provider == "telegram" && c.chat_id.is_none())
            || (matches!(input.provider.as_str(), "gotify") && c.api_base.is_none())
            || (input.provider == "ntfy" && c.topic.is_none())
        {
            return Err(ApiError::bad_request(
                "required notification credentials or destination are missing",
            ));
        }
    }
    Ok(())
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Message {
    pub title: String,
    pub body: String,
    pub event: String,
    pub time: String,
}
#[derive(Debug, Clone)]
pub struct Event {
    pub name: &'static str,
    pub key: String,
    pub indexer: String,
    pub job: String,
}
impl Event {
    pub fn new(name: &'static str, key: impl Into<String>) -> Self {
        Self {
            name,
            key: key.into(),
            indexer: String::new(),
            job: String::new(),
        }
    }
}
/// Single pass replacement: values are never reparsed as template source.
pub fn render(source: &str, vars: &[(&str, &str)]) -> String {
    let mut out = String::new();
    let mut rest = source;
    while let Some(start) = rest.find("{{") {
        out.push_str(&rest[..start]);
        rest = &rest[start..];
        let Some(end) = rest.find("}}") else {
            break;
        };
        let token = &rest[2..end];
        if let Some((_, v)) = vars.iter().find(|(k, _)| *k == token) {
            out.push_str(v);
        } else {
            out.push_str(&rest[..end + 2]);
        }
        rest = &rest[end + 2..];
    }
    out.push_str(rest);
    out
}
pub fn validate_template(title: &str, body: &str) -> Result<(), ApiError> {
    if title.trim().is_empty()
        || body.trim().is_empty()
        || title.len() > 500
        || body.len() > 4000
        || title.chars().any(char::is_control)
    {
        return Err(ApiError::bad_request(
            "template requires a title up to 500 bytes and a body up to 4000 bytes",
        ));
    }
    Ok(())
}
pub fn clip(s: &str, chars: usize) -> String {
    let mut used = 0;
    s.chars()
        .take_while(|ch| {
            used += ch.len_utf16();
            used <= chars
        })
        .collect()
}

/// Byte limits used by ntfy must also preserve UTF-8 boundaries.
pub fn clip_bytes(s: &str, limit: usize) -> &str {
    let mut end = s.len().min(limit);
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}
