//! Import boundaries for supported third-party definition formats.

use indexloom_definitions::IndexerDefinition;
use thiserror::Error;

/// Explicit upstream format selected by the user.
#[derive(Clone, Copy, Debug)]
pub enum SourceFormat {
    /// `Jackett Cardigann` definitions.
    Jackett,
    /// `Prowlarr Cardigann` definitions.
    Prowlarr,
}

/// Converts an upstream document into the canonical `IndexLoom` model.
pub trait DefinitionImporter {
    /// Imports one source document without persisting it.
    ///
    /// # Errors
    ///
    /// Returns [`ImportError`] when the document is malformed or contains
    /// behavior the selected adapter cannot represent safely.
    fn import(&self, source: &str) -> Result<IndexerDefinition, ImportError>;
}

/// Import failures retain enough context for a user-facing report.
#[derive(Debug, Error)]
pub enum ImportError {
    /// The upstream format is valid but unsupported by the current adapter.
    #[error("unsupported upstream feature: {0}")]
    Unsupported(String),
    /// The upstream document is malformed.
    #[error("invalid upstream definition: {0}")]
    Invalid(String),
}
