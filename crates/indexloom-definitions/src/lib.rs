//! Canonical, versioned `IndexLoom` definition model.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Current pre-stable schema identifier.
pub const API_VERSION: &str = "indexloom.io/v1alpha1";

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
    /// Optional summary.
    pub description: Option<String>,
    /// Definition language tag.
    pub language: Option<String>,
}

/// Runtime-neutral behavior consumed by the engine.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct IndexerSpec {
    /// Access model of the upstream indexer.
    pub privacy: Privacy,
    /// Candidate upstream base URLs.
    pub base_urls: Vec<String>,
    /// Supported normalized search capabilities.
    #[serde(default)]
    pub capabilities: Vec<String>,
    /// User-configurable fields.
    #[serde(default)]
    pub settings: Vec<SettingDefinition>,
    /// Search request and extraction rules.
    pub search: SearchDefinition,
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

/// One user-facing setting requested by a definition.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SettingDefinition {
    /// Stable key stored in the database.
    pub key: String,
    /// Label displayed in the UI.
    pub label: String,
    /// Input type understood by the frontend.
    #[serde(rename = "type")]
    pub value_type: SettingType,
    /// Whether the value is required.
    #[serde(default)]
    pub required: bool,
    /// Whether the value must be treated as a secret.
    #[serde(default)]
    pub secret: bool,
}

/// Supported configuration input types.
#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum SettingType {
    /// Single-line string.
    Text,
    /// Masked secret.
    Password,
    /// Boolean toggle.
    Boolean,
    /// Numeric value.
    Number,
}

/// Search request and HTML extraction rules.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SearchDefinition {
    /// HTTP method used for search.
    pub method: HttpMethod,
    /// Path relative to the selected base URL.
    pub path: String,
    /// Query or form parameters expressed as templates.
    #[serde(default)]
    pub inputs: BTreeMap<String, String>,
    /// CSS selector for result rows.
    pub rows: String,
    /// Named extraction rules for normalized result fields.
    pub fields: BTreeMap<String, FieldSelector>,
}

/// HTTP methods initially accepted by the schema.
#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum HttpMethod {
    /// HTTP GET.
    Get,
    /// HTTP POST.
    Post,
}

/// CSS-based field extraction.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FieldSelector {
    /// CSS selector relative to a result row.
    pub selector: String,
    /// Attribute to read, or `text` for text content.
    #[serde(default = "default_text_attribute")]
    pub attribute: String,
}

fn default_text_attribute() -> String {
    "text".to_owned()
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
    if definition.metadata.id.is_empty()
        || !definition.metadata.id.chars().all(|character| {
            character.is_ascii_lowercase() || character.is_ascii_digit() || character == '-'
        })
    {
        return Err(DefinitionError::Invalid(
            "metadata.id must contain lowercase ASCII letters, digits or hyphens".to_owned(),
        ));
    }
    if definition.spec.base_urls.is_empty() {
        return Err(DefinitionError::Invalid(
            "spec.baseUrls must not be empty".to_owned(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{API_VERSION, parse_yaml};

    #[test]
    fn parses_minimal_definition() {
        let source = r"
apiVersion: indexloom.io/v1alpha1
kind: Indexer
metadata:
  id: example
  name: Example
spec:
  privacy: public
  baseUrls:
    - https://example.invalid
  search:
    method: GET
    path: /search
    rows: .result
    fields:
      title:
        selector: .title
";

        let definition = parse_yaml(source).expect("definition should parse");
        assert_eq!(definition.api_version, API_VERSION);
        assert_eq!(definition.metadata.id, "example");
    }
}
