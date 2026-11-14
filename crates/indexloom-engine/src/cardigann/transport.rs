//! Definition-scoped encodings and a serialized request-start gate.
use super::{Duration, EngineError, Url};
use encoding_rs::{Encoding, UTF_8, UTF_16BE, UTF_16LE};

#[derive(Debug)]
pub(super) struct WireEncoding {
    codec: &'static Encoding,
    latin1: bool,
}
impl WireEncoding {
    pub(super) fn new(label: &str) -> Result<Self, EngineError> {
        let label = label.trim();
        let codec = Encoding::for_label(label.as_bytes()).ok_or(
            EngineError::CardigannIncompatible("unknown response/request encoding"),
        )?;
        Ok(Self {
            codec,
            latin1: matches!(
                label.to_ascii_lowercase().as_str(),
                "iso-8859-1" | "latin1" | "latin-1"
            ),
        })
    }
    pub(super) fn decode(&self, bytes: &[u8]) -> Result<String, EngineError> {
        if self.latin1 {
            return Ok(bytes.iter().map(|b| char::from(*b)).collect());
        }
        let (value, _, errors) = self.codec.decode(bytes);
        if errors {
            return Err(EngineError::CardigannIncompatible(
                "response is invalid in the declared encoding",
            ));
        }
        Ok(value.into_owned())
    }
    pub(super) fn encode(&self, value: &str) -> Result<Vec<u8>, EngineError> {
        if self.latin1 {
            return value
                .chars()
                .map(|c| {
                    u8::try_from(u32::from(c)).map_err(|_| {
                        EngineError::CardigannIncompatible(
                            "request text is not representable in ISO-8859-1",
                        )
                    })
                })
                .collect();
        }
        if self.codec == UTF_16LE {
            return Ok(value.encode_utf16().flat_map(u16::to_le_bytes).collect());
        }
        if self.codec == UTF_16BE {
            return Ok(value.encode_utf16().flat_map(u16::to_be_bytes).collect());
        }
        if self.codec == UTF_8 {
            return Ok(value.as_bytes().to_vec());
        }
        let (value, _, errors) = self.codec.encode(value);
        if errors {
            return Err(EngineError::CardigannIncompatible(
                "request text is not representable in the declared encoding",
            ));
        }
        Ok(value.into_owned())
    }
    pub(super) fn escape(&self, value: &str) -> Result<String, EngineError> {
        Ok(url::form_urlencoded::byte_serialize(&self.encode(value)?).collect())
    }
    pub(super) fn form(&self, values: &[(String, String)]) -> Result<String, EngineError> {
        values
            .iter()
            .map(|(k, v)| Ok(format!("{}={}", self.escape(k)?, self.escape(v)?)))
            .collect::<Result<Vec<_>, EngineError>>()
            .map(|v| v.join("&"))
    }
    pub(super) fn query(
        &self,
        url: &mut Url,
        values: &[(String, String)],
        raw: Option<&str>,
    ) -> Result<(), EngineError> {
        let mut parts = Vec::new();
        if let Some(q) = url.query().filter(|v| !v.is_empty()) {
            parts.push(q.to_owned());
        }
        if !values.is_empty() {
            parts.push(self.form(values)?);
        }
        if let Some(raw) = raw.filter(|v| !v.is_empty()) {
            parts.push(raw.trim_start_matches(['?', '&']).to_owned());
        }
        if !parts.is_empty() {
            url.set_query(Some(&parts.join("&")));
        }
        Ok(())
    }
}

#[derive(Debug)]
pub(super) struct RequestGate {
    delay: Duration,
    last: tokio::sync::Mutex<Option<tokio::time::Instant>>,
}
impl RequestGate {
    pub(super) fn new(seconds: Option<f64>) -> Result<Self, EngineError> {
        if seconds.is_some_and(|s| s > 86_400.0) {
            return Err(EngineError::CardigannIncompatible(
                "request delay exceeds one day",
            ));
        }
        let delay = Duration::try_from_secs_f64(seconds.unwrap_or(0.0)).map_err(|_| {
            EngineError::CardigannIncompatible("request delay must be finite and nonnegative")
        })?;
        Ok(Self {
            delay,
            last: tokio::sync::Mutex::new(None),
        })
    }
    pub(super) async fn wait(&self) {
        // Keep the lock across the wait so concurrent searches cannot reserve the same slot.
        let mut last = self.last.lock().await;
        if let Some(previous) = *last {
            tokio::time::sleep_until(previous + self.delay).await;
        }
        *last = Some(tokio::time::Instant::now());
    }
}

pub(super) fn certificate_roots(
    values: &[String],
) -> Result<Vec<reqwest::Certificate>, EngineError> {
    if values.len() > 16 {
        return Err(EngineError::CardigannIncompatible(
            "more than 16 certificate bundles",
        ));
    }
    let mut roots = Vec::new();
    for value in values {
        if value.len() > 256 * 1024
            || !value
                .trim_start()
                .starts_with("-----BEGIN CERTIFICATE-----")
        {
            return Err(EngineError::CardigannIncompatible(
                "legacy certificate fingerprints are unsupported; supply a PEM CA certificate with normal TLS validation",
            ));
        }
        let certificates = reqwest::Certificate::from_pem_bundle(value.as_bytes())
            .map_err(|_| EngineError::CardigannIncompatible("invalid PEM CA certificate"))?;
        if certificates.is_empty() {
            return Err(EngineError::CardigannIncompatible(
                "empty PEM CA certificate bundle",
            ));
        }
        roots.extend(certificates);
    }
    Ok(roots)
}
