//! Canonical, versioned `IndexLoom` definition model.

pub mod capabilities;

use serde::{Deserialize, Serialize};
use serde_yaml::Value;
use thiserror::Error;

/// Current pre-stable schema identifier.
pub const API_VERSION: &str = "indexloom.io/v1alpha1";
/// Reproducible catalog converter identifier.
pub const GENERATED_BY: &str = "indexloom-importer/v1";
/// License carried by converted Cardigann definitions.
pub const CARDIGANN_LICENSE: &str = "GPL-2.0-only";

/// A complete YAML indexer definition.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct IndexerDefinition {
    /// Version of the `IndexLoom` definition schema.
    pub api_version: String,
    /// Resource kind. Currently always `Indexer`.
    pub kind: String,
    /// Human and machine-readable identity.
    pub metadata: Metadata,
    /// Verifiable source and conversion metadata.
    pub provenance: Provenance,
    /// Executable indexer behavior.
    pub spec: IndexerSpec,
}

/// Definition identity and documentation.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Metadata {
    /// Stable lowercase identifier.
    pub id: String,
    /// Display name.
    pub name: String,
    /// Human-readable summary.
    pub description: String,
    /// Definition language tag.
    pub language: String,
}

/// Reproducibility metadata for a converted definition.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Provenance {
    /// Deterministic converter and format revision.
    pub generated_by: String,
    /// Exact upstream source document.
    pub source: SourceProvenance,
}

/// Immutable origin of one converted definition.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SourceProvenance {
    /// Upstream project identifier.
    pub project: SourceProject,
    /// Canonical upstream Git repository URL.
    pub repository: String,
    /// Full upstream Git commit SHA.
    pub revision: String,
    /// Source path relative to the repository root.
    pub path: String,
    /// Original identifier before canonicalization.
    pub source_id: String,
    /// SPDX license identifier inherited by the generated definition.
    pub license: String,
    /// SHA-256 of the exact source file bytes.
    pub file_sha256: String,
    /// SHA-256 of the canonicalized source document.
    pub content_sha256: String,
}

/// Supported catalog origins.
#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum SourceProject {
    /// Jackett's Cardigann definition catalog.
    Jackett,
    /// Prowlarr's Cardigann compatibility catalog.
    ProwlarrIndexers,
    /// User-supplied protocol definitions, distinct from GPL tracker catalog.
    User,
}

/// Runtime-neutral definition envelope.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct IndexerSpec {
    /// Result transport exposed by the indexer.
    pub protocol: Protocol,
    /// Access model of the upstream indexer.
    pub privacy: Privacy,
    /// Candidate current upstream base URLs.
    pub base_urls: Vec<String>,
    /// Historical or fallback upstream URLs.
    #[serde(default)]
    pub legacy_urls: Vec<String>,
    /// Definition identifiers superseded by this definition.
    #[serde(default)]
    pub replaced_ids: Vec<String>,
    /// Engine-specific declarative implementation.
    pub implementation: CardigannImplementation,
}

/// Supported result protocols.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Protocol {
    /// `BitTorrent` metadata and releases.
    Torrent,
    Usenet,
}

/// Upstream access model.
#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Privacy {
    /// No account is required.
    Public,
    /// Registration is available to the public.
    SemiPrivate,
    /// An account or invitation is required.
    Private,
}

/// Lossless Cardigann compatibility document inside the `IndexLoom` envelope.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CardigannImplementation {
    /// Declarative execution engine.
    pub engine: DefinitionEngine,
    /// Source response encoding.
    pub encoding: String,
    /// Whether HTTP redirects may be followed after policy checks.
    pub follow_redirects: Option<bool>,
    /// Whether download links should be tested as torrents.
    pub test_link_torrent: Option<bool>,
    /// Minimum delay between requests.
    pub request_delay_seconds: Option<f64>,
    /// Explicit certificate material used by a small number of definitions.
    #[serde(default)]
    pub certificates: Vec<String>,
    /// Search modes and category mappings.
    pub capabilities: Value,
    /// User-facing configuration declarations.
    #[serde(default)]
    pub settings: Vec<Value>,
    /// Optional authentication workflow.
    pub authentication: Option<Value>,
    /// Optional secondary download-page extraction workflow.
    pub download: Option<Value>,
    /// Search requests, selectors, filters, and normalized result fields.
    pub search: Value,
}

/// Supported declarative execution engines.
#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum DefinitionEngine {
    /// Cardigann-compatible request and extraction behavior.
    Cardigann,
    /// Native Newznab XML query and NZB download adapter.
    Newznab,
}

/// User-facing configuration contract derived from one definition.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigurationSchema {
    /// Editable definition fields, excluding informational notices.
    pub fields: Vec<ConfigurationField>,
    /// The definition explicitly declares that `FlareSolverr` may be needed.
    pub flaresolverr_recommended: bool,
    /// The definition declares a dedicated browser `User-Agent` setting.
    pub user_agent_recommended: bool,
}

/// One editable configuration field derived from Cardigann settings.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigurationField {
    /// Stable key referenced by Cardigann templates.
    pub name: String,
    /// Human-readable field label.
    pub label: String,
    /// Input control expected by the frontend.
    pub kind: ConfigurationFieldKind,
    /// Whether the value is encrypted and never returned by the API.
    pub secret: bool,
    /// Optional scalar default from the source definition.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_value: Option<ConfigurationValue>,
    /// Allowed values for select controls.
    pub options: Vec<ConfigurationOption>,
}

/// Supported editable Cardigann control types.
#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ConfigurationFieldKind {
    /// Single-line text.
    Text,
    /// Password-like secret.
    Password,
    /// Boolean toggle.
    Checkbox,
    /// Select from a finite list.
    Select,
}

/// Scalar value accepted by a configuration field.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(untagged)]
pub enum ConfigurationValue {
    /// Text value.
    String(String),
    /// Boolean value.
    Boolean(bool),
    /// Integer value.
    Integer(i64),
    /// Floating-point value.
    Number(f64),
}

/// One select option, with its source value preserved as text.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigurationOption {
    /// Value rendered into Cardigann templates.
    pub value: String,
    /// Human-readable option label.
    pub label: String,
}

impl IndexerDefinition {
    /// Derives editable fields and browser-transport requirements.
    #[must_use]
    pub fn configuration_schema(&self) -> ConfigurationSchema {
        let mut fields = Vec::new();
        let mut flaresolverr_recommended = false;
        let mut user_agent_recommended = false;

        for setting in &self.spec.implementation.settings {
            let Value::Object(mapping) = setting else {
                continue;
            };
            let Some(setting_type) = mapping_string(mapping, "type") else {
                continue;
            };
            if setting_type == "info_flaresolverr" {
                flaresolverr_recommended = true;
                continue;
            }
            if setting_type == "info_useragent" {
                user_agent_recommended = true;
                continue;
            }
            if setting_type.starts_with("info") {
                continue;
            }

            let Some(name) = mapping_string(mapping, "name") else {
                continue;
            };
            if name.eq_ignore_ascii_case("useragent") {
                user_agent_recommended = true;
                continue;
            }
            let Some(kind) = configuration_kind(&setting_type) else {
                continue;
            };
            let label = mapping_string(mapping, "label").unwrap_or_else(|| name.clone());
            let default_value = mapping.get("default").and_then(configuration_value);
            let options = mapping
                .get("options")
                .map(configuration_options)
                .unwrap_or_default();
            fields.push(ConfigurationField {
                secret: matches!(kind, ConfigurationFieldKind::Password) || is_secret_name(&name),
                name,
                label,
                kind,
                default_value,
                options,
            });
        }

        ConfigurationSchema {
            fields,
            flaresolverr_recommended,
            user_agent_recommended,
        }
    }
}

fn configuration_kind(setting_type: &str) -> Option<ConfigurationFieldKind> {
    match setting_type {
        "text" => Some(ConfigurationFieldKind::Text),
        "password" => Some(ConfigurationFieldKind::Password),
        "checkbox" => Some(ConfigurationFieldKind::Checkbox),
        "select" => Some(ConfigurationFieldKind::Select),
        _ => None,
    }
}

fn mapping_string(mapping: &serde_yaml::Mapping, key: &str) -> Option<String> {
    mapping.get(key).and_then(scalar_string)
}

fn scalar_string(value: &Value) -> Option<String> {
    match value {
        Value::String(value) => Some(value.clone()),
        Value::Bool(value) => Some(value.to_string()),
        Value::Number(value) => Some(value.to_string()),
        _ => None,
    }
}

fn configuration_value(value: &Value) -> Option<ConfigurationValue> {
    match value {
        Value::String(value) => Some(ConfigurationValue::String(value.clone())),
        Value::Bool(value) => Some(ConfigurationValue::Boolean(*value)),
        Value::Number(value) => value
            .as_i64()
            .map(ConfigurationValue::Integer)
            .or_else(|| value.as_f64().map(ConfigurationValue::Number)),
        _ => None,
    }
}

fn configuration_options(value: &Value) -> Vec<ConfigurationOption> {
    let Value::Object(mapping) = value else {
        return Vec::new();
    };
    mapping
        .iter()
        .filter_map(|(value, label)| {
            Some(ConfigurationOption {
                value: value.clone(),
                label: scalar_string(label)?,
            })
        })
        .collect()
}

fn is_secret_name(name: &str) -> bool {
    let normalized = name.to_ascii_lowercase();
    [
        "password", "passwd", "pass", "cookie", "token", "key", "pin", "2fa", "otp",
    ]
    .iter()
    .any(|marker| normalized.contains(marker))
}

/// Definition parsing and validation errors.
#[derive(Debug, Error)]
pub enum DefinitionError {
    /// YAML could not be deserialized.
    #[error("invalid YAML definition: {0}")]
    InvalidYaml(#[from] serde_yaml::Error),
    /// The definition uses an unsupported schema version.
    #[error("unsupported definition API version: {0}")]
    UnsupportedVersion(String),
    /// A required invariant is not satisfied.
    #[error("invalid definition: {0}")]
    Invalid(String),
}

/// Parses a YAML definition and enforces inexpensive structural invariants.
///
/// # Errors
///
/// Returns [`DefinitionError`] when the source is malformed, uses an
/// unsupported schema version, or violates a required invariant.
pub fn parse_yaml(source: &str) -> Result<IndexerDefinition, DefinitionError> {
    let definition: IndexerDefinition = serde_yaml::from_str(source)?;
    validate(&definition)?;
    Ok(definition)
}

/// Validates invariants not represented by Serde.
///
/// # Errors
///
/// Returns [`DefinitionError`] when the definition uses an unsupported schema
/// version or violates a required invariant.
pub fn validate(definition: &IndexerDefinition) -> Result<(), DefinitionError> {
    if definition.api_version != API_VERSION {
        return Err(DefinitionError::UnsupportedVersion(
            definition.api_version.clone(),
        ));
    }
    if definition.kind != "Indexer" {
        return Err(DefinitionError::Invalid("kind must be Indexer".to_owned()));
    }
    if !is_canonical_id(&definition.metadata.id) {
        return Err(DefinitionError::Invalid(
            "metadata.id must be lowercase kebab-case ASCII".to_owned(),
        ));
    }
    if definition.metadata.name.is_empty() || definition.metadata.language.is_empty() {
        return Err(DefinitionError::Invalid(
            "metadata.name and metadata.language must not be empty".to_owned(),
        ));
    }
    if definition.spec.base_urls.is_empty()
        || definition
            .spec
            .base_urls
            .iter()
            .any(|url| !(url.starts_with("http://") || url.starts_with("https://")))
    {
        return Err(DefinitionError::Invalid(
            "spec.baseUrls must contain HTTP(S) URLs".to_owned(),
        ));
    }
    let source = &definition.provenance.source;
    let custom = matches!(source.project, SourceProject::User);
    if (custom
        && (definition.provenance.generated_by != "indexloom-user/v1"
            || source.license != "NOASSERTION"))
        || (!custom && definition.provenance.generated_by != GENERATED_BY)
    {
        return Err(DefinitionError::Invalid(
            "invalid definition provenance".to_owned(),
        ));
    }
    if !custom && source.license != CARDIGANN_LICENSE {
        return Err(DefinitionError::Invalid(
            "converted Cardigann definitions must retain GPL-2.0-only".to_owned(),
        ));
    }
    if !source.repository.starts_with("https://github.com/")
        || source.path.is_empty()
        || source.path.starts_with('/')
        || source.path.split('/').any(|part| part == "..")
    {
        return Err(DefinitionError::Invalid(
            "invalid provenance source location".to_owned(),
        ));
    }
    if !is_lower_hex(&source.revision, 40)
        || !is_lower_hex(&source.file_sha256, 64)
        || !is_lower_hex(&source.content_sha256, 64)
    {
        return Err(DefinitionError::Invalid(
            "invalid provenance revision or SHA-256".to_owned(),
        ));
    }
    validate_protocol_profile(definition, custom)?;
    let encoding = definition.spec.implementation.encoding.trim();
    if encoding.is_empty() || encoding.len() > 64 || !encoding.is_ascii() {
        return Err(DefinitionError::Invalid(
            "encoding must be a nonempty ASCII label of at most 64 bytes".to_owned(),
        ));
    }
    if let Some(delay) = definition.spec.implementation.request_delay_seconds
        && (!delay.is_finite() || delay.is_sign_negative() || delay > 86_400.0)
    {
        return Err(DefinitionError::Invalid(
            "requestDelaySeconds must be finite and between 0 and 86400".to_owned(),
        ));
    }
    if !matches!(
        definition.spec.implementation.capabilities,
        Value::Object(_)
    ) || !matches!(definition.spec.implementation.search, Value::Object(_))
    {
        return Err(DefinitionError::Invalid(
            "Cardigann capabilities and search must be mappings".to_owned(),
        ));
    }
    Ok(())
}

fn validate_protocol_profile(
    definition: &IndexerDefinition,
    custom: bool,
) -> Result<(), DefinitionError> {
    match (
        definition.spec.protocol,
        definition.spec.implementation.engine,
    ) {
        (Protocol::Torrent, DefinitionEngine::Cardigann)
        | (Protocol::Usenet, DefinitionEngine::Newznab) => {}
        _ => {
            return Err(DefinitionError::Invalid(
                "protocol and definition engine are incompatible".to_owned(),
            ));
        }
    }
    if custom
        && (definition.spec.protocol != Protocol::Usenet
            || !definition.metadata.id.starts_with("usenet-"))
    {
        return Err(DefinitionError::Invalid(
            "unsupported custom definition".to_owned(),
        ));
    }
    Ok(())
}

fn is_canonical_id(value: &str) -> bool {
    !value.is_empty()
        && value.split('-').all(|part| {
            !part.is_empty()
                && part
                    .chars()
                    .all(|character| character.is_ascii_lowercase() || character.is_ascii_digit())
        })
}

fn is_lower_hex(value: &str, length: usize) -> bool {
    value.len() == length
        && value
            .chars()
            .all(|character| character.is_ascii_digit() || ('a'..='f').contains(&character))
}

#[cfg(test)]
mod tests {
    use super::{API_VERSION, parse_yaml, validate};

    const EXAMPLE: &str = r#"
apiVersion: indexloom.io/v1alpha1
kind: Indexer
metadata:
  id: example
  name: Example
  description: Example definition
  language: en-US
provenance:
  generatedBy: indexloom-importer/v1
  source:
    project: jackett
    repository: https://github.com/Jackett/Jackett
    revision: "0000000000000000000000000000000000000000"
    path: src/Jackett.Common/Definitions/example.yml
    sourceId: example
    license: GPL-2.0-only
    fileSha256: "0000000000000000000000000000000000000000000000000000000000000000"
    contentSha256: "1111111111111111111111111111111111111111111111111111111111111111"
spec:
  protocol: torrent
  privacy: public
  baseUrls:
    - https://example.invalid/
  implementation:
    engine: cardigann
    encoding: UTF-8
    capabilities: {}
    settings: []
    search:
      paths:
        - path: /search
      rows:
        selector: .result
      fields:
        title:
          selector: .title
"#;

    #[test]
    fn parses_converted_definition() {
        let source = EXAMPLE;

        let definition = parse_yaml(source).expect("definition should parse");
        assert_eq!(definition.api_version, API_VERSION);
        assert_eq!(definition.metadata.id, "example");
        assert_eq!(definition.provenance.source.source_id, "example");
    }

    #[test]
    fn derives_secrets_and_browser_transport_hints() {
        let source = r#"
apiVersion: indexloom.io/v1alpha1
kind: Indexer
metadata:
  id: private-example
  name: Private example
  description: Example definition
  language: en-US
provenance:
  generatedBy: indexloom-importer/v1
  source:
    project: jackett
    repository: https://github.com/Jackett/Jackett
    revision: "0000000000000000000000000000000000000000"
    path: src/Jackett.Common/Definitions/private-example.yml
    sourceId: private-example
    license: GPL-2.0-only
    fileSha256: "0000000000000000000000000000000000000000000000000000000000000000"
    contentSha256: "1111111111111111111111111111111111111111111111111111111111111111"
spec:
  protocol: torrent
  privacy: private
  baseUrls: [https://example.invalid/]
  implementation:
    engine: cardigann
    encoding: UTF-8
    capabilities: {}
    settings:
      - name: username
        type: text
        label: Username
        default: guest
      - name: password
        type: password
        label: Password
      - name: cookie
        type: text
        label: Cookie
      - name: useragent
        type: text
        label: User-Agent
      - name: info_flaresolverr
        type: info_flaresolverr
    search: { paths: [{ path: /search }], rows: { selector: .result }, fields: {} }
"#;

        let definition = parse_yaml(source).expect("definition should parse");
        let schema = definition.configuration_schema();
        assert!(schema.flaresolverr_recommended);
        assert!(schema.user_agent_recommended);
        assert_eq!(schema.fields.len(), 3);
        assert!(!schema.fields[0].secret);
        assert!(schema.fields[1].secret);
        assert!(schema.fields[2].secret);
        let with_default = serde_json::to_value(&schema.fields[0]).expect("field should serialize");
        let without_default =
            serde_json::to_value(&schema.fields[1]).expect("field should serialize");
        assert_eq!(with_default["defaultValue"], "guest");
        assert!(without_default.get("defaultValue").is_none());
    }
    #[test]
    fn preserves_encoding_delay_certificates_and_dynamic_search_options() {
        for label in ["UTF-8", "windows-1251", "windows-1252", "ISO-8859-1"] {
            let mut definition = parse_yaml(EXAMPLE).unwrap();
            definition.spec.implementation.encoding = label.to_owned();
            definition.spec.implementation.request_delay_seconds = Some(0.25);
            definition.spec.implementation.certificates = vec!["legacy-fingerprint".into()];
            definition.spec.implementation.search = serde_yaml::from_str("paths: [{path: /search, inputs: {offset: '{{ .Query.Offset }}'}}]\npreprocessingfilters: [{name: htmldecode}]\nrows: {selector: data, multiple: true}\nfields: {}\n").unwrap();
            let source = serde_yaml::to_string(&definition).unwrap();
            let parsed = parse_yaml(&source).unwrap();
            assert_eq!(parsed.spec.implementation.encoding, label);
            assert_eq!(parsed.spec.implementation.request_delay_seconds, Some(0.25));
            assert_eq!(
                parsed.spec.implementation.certificates,
                ["legacy-fingerprint"]
            );
            assert_eq!(
                parsed.spec.implementation.search,
                definition.spec.implementation.search
            );
        }
    }
    #[test]
    fn rejects_invalid_encoding_labels_and_unbounded_request_delays() {
        let mut definition = parse_yaml(EXAMPLE).unwrap();
        for label in ["", "   ", "кириллица"] {
            definition.spec.implementation.encoding = label.into();
            assert!(validate(&definition).is_err());
        }
        definition.spec.implementation.encoding = "UTF-8".into();
        for delay in [f64::NAN, f64::INFINITY, -0.1, 86_401.0] {
            definition.spec.implementation.request_delay_seconds = Some(delay);
            assert!(validate(&definition).is_err());
        }
    }
}
