//! Provider adapters share one bounded HTTP transport. No response/error text leaves this module.
use super::model::{Config, Message, clip, clip_bytes};
use base64::Engine;
use indexloom_storage::notifications::Channel;
use reqwest::{Client, RequestBuilder};
use serde_json::{Value, json};
use std::{collections::BTreeMap, time::Duration};

#[derive(Debug, Clone, Copy)]
pub struct Failure {
    pub summary: &'static str,
    pub retryable: bool,
}
impl Failure {
    fn permanent(summary: &'static str) -> Self {
        Self {
            summary,
            retryable: false,
        }
    }
}
pub fn client() -> Result<Client, reqwest::Error> {
    Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .no_proxy()
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(15))
        .user_agent("IndexLoom-notifications")
        .build()
}
fn secret<'a>(s: &'a BTreeMap<String, String>, key: &str) -> Result<&'a str, Failure> {
    s.get(key)
        .map(String::as_str)
        .ok_or_else(|| Failure::permanent("Missing credentials"))
}
fn append(base: &str, segments: &[&str]) -> Result<url::Url, Failure> {
    let mut url = url::Url::parse(base).map_err(|_| Failure::permanent("Invalid destination"))?;
    url.path_segments_mut()
        .map_err(|()| Failure::permanent("Invalid destination"))?
        .pop_if_empty()
        .extend(segments);
    Ok(url)
}
pub(super) fn request(
    client: &Client,
    provider: &str,
    c: &Config,
    s: &BTreeMap<String, String>,
    m: &Message,
) -> Result<RequestBuilder, Failure> {
    let text = format!("{}\n{}", m.title, m.body);
    Ok(match provider {
        "telegram"=>{
            let url=append(c.api_base.as_deref().unwrap_or("https://api.telegram.org"),&[&format!("bot{}",secret(s,"token")?),"sendMessage"])?;
            let mut body=json!({"chat_id":c.chat_id,"text":clip(&text,4096),"disable_notification":c.disable_notification,"link_preview_options":{"is_disabled":true}});
            if let Some(thread)=c.thread_id {body["message_thread_id"]=json!(thread);}
            client.post(url).json(&body)
        },
        "discord"=>client.post(secret(s,"url")?).json(&json!({"content":clip(&text,2000),"allowed_mentions":{"parse":[]}})),
        "slack"=>client.post(secret(s,"url")?).json(&json!({"text":clip(&text,4000),"mrkdwn":false,"unfurl_links":false,"unfurl_media":false})),
        "webhook"=>{
            let r=client.post(secret(s,"url")?).json(&json!({"title":m.title,"body":m.body,"event":m.event,"time":m.time}));
            if let Some(auth)=s.get("bearer") {r.bearer_auth(auth)} else {r}
        },
        "ntfy"=>{
            let url=append(c.api_base.as_deref().unwrap_or("https://ntfy.sh"),&[c.topic.as_deref().ok_or_else(|| Failure::permanent("Missing topic"))?])?;
            // RFC 2047 title supports UTF-8 without illegal raw HTTP header bytes.
            let title=format!("=?UTF-8?B?{}?=",base64::prelude::BASE64_STANDARD.encode(clip(&m.title,200)));
            let r=client.post(url).header("Title",title).header("Priority",c.priority.unwrap_or(3).to_string()).header("Content-Type","text/plain; charset=utf-8").body(clip_bytes(&m.body, 4096).to_owned());
            if let Some(token)=s.get("token") {r.bearer_auth(token)} else {r}
        },
        "gotify"=>client.post(append(c.api_base.as_deref().ok_or_else(|| Failure::permanent("Missing API base"))?,&["message"])?)
            .header("X-Gotify-Key",secret(s,"token")?).json(&json!({"title":clip(&m.title,200),"message":clip(&m.body,4000),"priority":c.priority.unwrap_or(5)})),
        "apprise"=>client.post(append(secret(s,"url")?,&["notify",secret(s,"key")?,""])?)
            .json(&json!({"title":clip(&m.title,200),"body":clip(&m.body,4000),"format":"text"})),
        "pushover"=>client.post("https://api.pushover.net/1/messages.json").form(&[
            ("token",secret(s,"token")?.to_owned()),("user",secret(s,"user")?.to_owned()),
            ("title",clip(&m.title,250)),("message",clip(&m.body,1024)),("priority",c.priority.unwrap_or(0).to_string()),
        ]),
        _=>return Err(Failure::permanent("Unsupported provider")),
    })
}
pub async fn send(client: &Client, c: &Channel, m: &Message) -> Result<(), Failure> {
    let config: Config =
        serde_json::from_str(&c.config).map_err(|_| Failure::permanent("Invalid configuration"))?;
    let response = request(client, &c.provider, &config, &c.secrets, m)?
        .send()
        .await
        .map_err(|e| Failure {
            summary: if e.is_timeout() {
                "Request timed out"
            } else {
                "Connection or TLS failure"
            },
            retryable: true,
        })?;
    let status = response.status();
    if !status.is_success() {
        return Err(Failure {
            summary: match status.as_u16() {
                401 | 403 => "Provider rejected credentials or permissions",
                429 => "Provider rate limit",
                300..=399 => "Provider redirect refused",
                500..=599 => "Provider server error",
                _ => "Provider rejected request",
            },
            retryable: status.as_u16() == 429 || status.is_server_error(),
        });
    }
    if matches!(c.provider.as_str(), "telegram" | "gotify" | "pushover") {
        let mut response = response;
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|_| Failure {
            summary: "Response read failed",
            retryable: true,
        })? {
            if bytes.len() + chunk.len() > 32768 {
                return Err(Failure::permanent("Provider response too large"));
            }
            bytes.extend_from_slice(&chunk);
        }
        validate_result(&c.provider, &bytes)?;
    } else if c.provider == "slack" {
        let mut response = response;
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|_| Failure {
            summary: "Response read failed",
            retryable: true,
        })? {
            if bytes.len() + chunk.len() > 1024 {
                return Err(Failure::permanent("Provider response too large"));
            }
            bytes.extend_from_slice(&chunk);
        }
        if bytes.as_slice() != b"ok" {
            return Err(Failure::permanent("Provider reported an API error"));
        }
    }
    Ok(())
}
fn validate_result(provider: &str, bytes: &[u8]) -> Result<(), Failure> {
    let value: Value = serde_json::from_slice(bytes)
        .map_err(|_| Failure::permanent("Invalid provider response"))?;
    let ok = match provider {
        "telegram" => value["ok"] == true,
        "pushover" => value["status"] == 1,
        "gotify" => value["id"].as_i64().is_some_and(|id| id > 0) && value.get("error").is_none(),
        _ => true,
    };
    if ok {
        Ok(())
    } else {
        let code = value
            .get("error_code")
            .or_else(|| value.get("errorCode"))
            .and_then(Value::as_u64)
            .unwrap_or(0);
        Err(Failure {
            summary: "Provider reported an API error",
            retryable: code == 429 || (500..=599).contains(&code),
        })
    }
}
