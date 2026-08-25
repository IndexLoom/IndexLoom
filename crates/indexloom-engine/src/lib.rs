//! Sandboxed HTTP and HTML execution primitives.

use reqwest::Client;
use scraper::{Html, Selector};
use thiserror::Error;

/// Shared HTTP execution engine.
#[derive(Clone, Debug)]
pub struct SearchEngine {
    client: Client,
}

impl SearchEngine {
    /// Builds a client with redirects disabled until definition policy is applied.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError`] when the TLS or HTTP client cannot be initialized.
    pub fn new() -> Result<Self, EngineError> {
        let client = Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .cookie_store(true)
            .build()?;
        Ok(Self { client })
    }

    /// Exposes the configured client to bounded request executors.
    pub const fn client(&self) -> &Client {
        &self.client
    }

    /// Extracts text values from an HTML document for conformance tests.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError`] when `selector` is not valid CSS.
    pub fn extract_text(html: &str, selector: &str) -> Result<Vec<String>, EngineError> {
        let selector = Selector::parse(selector)
            .map_err(|error| EngineError::InvalidSelector(error.to_string()))?;
        let document = Html::parse_document(html);
        Ok(document
            .select(&selector)
            .map(|element| element.text().collect::<String>().trim().to_owned())
            .collect())
    }
}

/// Engine construction and parsing errors.
#[derive(Debug, Error)]
pub enum EngineError {
    /// HTTP client construction failed.
    #[error("failed to build HTTP client: {0}")]
    HttpClient(#[from] reqwest::Error),
    /// A definition contains an invalid CSS selector.
    #[error("invalid CSS selector: {0}")]
    InvalidSelector(String),
}

#[cfg(test)]
mod tests {
    use super::SearchEngine;

    #[test]
    fn extracts_text_from_matching_nodes() {
        let values = SearchEngine::extract_text(
            r#"<div class="result">First</div><div class="result">Second</div>"#,
            ".result",
        )
        .expect("selector should be valid");

        assert_eq!(values, ["First", "Second"]);
    }
}
