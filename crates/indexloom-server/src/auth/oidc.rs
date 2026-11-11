//! `OpenID` Connect authorization code + PKCE with verified ID tokens.

#![allow(clippy::missing_errors_doc)]
#![allow(clippy::missing_fields_in_debug)]
//! No unsigned JWT parsing or trusted-forwarded-header shortcuts.

use std::{collections::BTreeSet, env, time::Duration};

use anyhow::{Context, Result, bail};
use openidconnect::{
    AuthorizationCode, ClientId, ClientSecret, CsrfToken, IssuerUrl, Nonce, PkceCodeChallenge,
    PkceCodeVerifier, RedirectUrl, Scope, TokenResponse,
    core::{CoreAuthenticationFlow, CoreClient, CoreProviderMetadata},
};
use serde::Serialize;

use indexloom_storage::Store;

use super::digest;

#[derive(Clone)]
pub struct OidcSettings {
    pub issuer: String,
    pub client_id: String,
    client_secret: Option<String>,
    redirect_uri: String,
    allowed_emails: BTreeSet<String>,
    owner_email: Option<String>,
}

impl std::fmt::Debug for OidcSettings {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OidcSettings")
            .field("issuer", &self.issuer)
            .field("client_id", &self.client_id)
            .field("secret", &"<redacted>")
            .field("allowlist_count", &self.allowed_emails.len())
            .field("owner_email_configured", &self.owner_email.is_some())
            .finish()
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StartResponse {
    pub authorization_url: String,
    pub state: String,
}

#[derive(Clone, Debug)]
pub struct VerifiedIdentity {
    pub email: String,
    pub subject: String,
    pub issuer: String,
}

impl OidcSettings {
    pub fn from_env() -> Result<Option<Self>> {
        let Some(issuer) = env::var("INDEXLOOM_OIDC_ISSUER")
            .ok()
            .filter(|s| !s.is_empty())
        else {
            return Ok(None);
        };
        let url = url::Url::parse(&issuer).context("OIDC issuer URL is malformed")?;
        if url.scheme() != "https"
            || url.host_str().is_none()
            || url.host_str().is_some_and(|host| {
                host.parse::<std::net::IpAddr>().is_ok()
                    || host == "localhost"
                    || host.to_ascii_lowercase().ends_with(".local")
            })
            || !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
        {
            bail!("OIDC issuer must be a public HTTPS URL without credentials");
        }
        let client_id =
            env::var("INDEXLOOM_OIDC_CLIENT_ID").context("INDEXLOOM_OIDC_CLIENT_ID is required")?;
        if client_id.is_empty() {
            bail!("OIDC client ID is empty");
        }
        let redirect_uri = env::var("INDEXLOOM_OIDC_REDIRECT_URI")
            .context("INDEXLOOM_OIDC_REDIRECT_URI is required")?;
        let parsed_redirect =
            url::Url::parse(&redirect_uri).context("OIDC redirect URI is malformed")?;
        if parsed_redirect.scheme() != "https"
            && !matches!(parsed_redirect.host_str(), Some("localhost" | "127.0.0.1"))
        {
            bail!("OIDC redirect requires HTTPS outside localhost");
        }
        if parsed_redirect.query().is_some() || parsed_redirect.fragment().is_some() {
            bail!("OIDC redirect must not include query or fragment");
        }
        let allowed_emails = env::var("INDEXLOOM_OIDC_ALLOWED_EMAILS")
            .unwrap_or_default()
            .split(',')
            .map(|email| email.trim().to_ascii_lowercase())
            .filter(|email| !email.is_empty())
            .collect::<BTreeSet<_>>();
        if allowed_emails.is_empty() {
            bail!("OIDC requires INDEXLOOM_OIDC_ALLOWED_EMAILS (no open signup)");
        }
        let owner_email = env::var("INDEXLOOM_OIDC_OWNER_EMAIL")
            .ok()
            .map(|email| email.trim().to_ascii_lowercase())
            .filter(|email| !email.is_empty());
        if owner_email.as_ref().is_some_and(|email| !allowed_emails.contains(email)) {
            bail!("INDEXLOOM_OIDC_OWNER_EMAIL must be in INDEXLOOM_OIDC_ALLOWED_EMAILS");
        }
        Ok(Some(Self {
            issuer,
            client_id,
            client_secret: super::read_optional_secret("INDEXLOOM_OIDC_CLIENT_SECRET")?,
            redirect_uri,
            allowed_emails,
            owner_email,
        }))
    }

    /// Checks whether a verified OIDC identity was explicitly designated as an owner.
    #[must_use]
    pub fn is_owner_email(&self, email: &str) -> bool {
        self.owner_email.as_deref() == Some(email)
    }

    /// Whether an OIDC owner is explicitly configured.
    #[must_use]
    pub fn has_owner_email(&self) -> bool {
        self.owner_email.is_some()
    }

    fn http_client() -> Result<openidconnect::reqwest::Client> {
        Ok(openidconnect::reqwest::Client::builder()
            .redirect(openidconnect::reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(15))
            .build()?)
    }

    pub async fn begin(&self, store: &Store) -> Result<StartResponse> {
        let http_client = Self::http_client()?;
        let metadata = CoreProviderMetadata::discover_async(
            IssuerUrl::new(self.issuer.clone())?,
            &http_client,
        )
        .await
        .context("OIDC issuer discovery failed")?;
        let client = CoreClient::from_provider_metadata(
            metadata,
            ClientId::new(self.client_id.clone()),
            self.client_secret.clone().map(ClientSecret::new),
        )
        .set_redirect_uri(RedirectUrl::new(self.redirect_uri.clone())?);
        let (challenge, verifier) = PkceCodeChallenge::new_random_sha256();
        let (url, state, nonce) = client
            .authorize_url(
                CoreAuthenticationFlow::AuthorizationCode,
                CsrfToken::new_random,
                Nonce::new_random,
            )
            .add_scope(Scope::new("email".to_owned()))
            .set_pkce_challenge(challenge)
            .url();
        store
            .store_oidc_flow(&digest(state.secret()), nonce.secret(), verifier.secret())
            .await?;
        Ok(StartResponse {
            authorization_url: url.to_string(),
            state: state.secret().to_owned(),
        })
    }

    pub async fn finish(&self, store: &Store, state: &str, code: &str) -> Result<VerifiedIdentity> {
        let (nonce, verifier) = store
            .consume_oidc_flow(&digest(state))
            .await?
            .context("OIDC state was expired, invalid or already consumed")?;
        let http_client = Self::http_client()?;
        let metadata = CoreProviderMetadata::discover_async(
            IssuerUrl::new(self.issuer.clone())?,
            &http_client,
        )
        .await
        .context("OIDC issuer discovery failed")?;
        let client = CoreClient::from_provider_metadata(
            metadata,
            ClientId::new(self.client_id.clone()),
            self.client_secret.clone().map(ClientSecret::new),
        )
        .set_redirect_uri(RedirectUrl::new(self.redirect_uri.clone())?);
        let response = client
            .exchange_code(AuthorizationCode::new(code.to_owned()))?
            .set_pkce_verifier(PkceCodeVerifier::new(verifier))
            .request_async(&http_client)
            .await
            .context("OIDC authorization code exchange failed")?;
        let id_token = response
            .id_token()
            .context("OIDC provider omitted ID token")?;
        let claims = id_token
            .claims(&client.id_token_verifier(), &Nonce::new(nonce))
            .context("OIDC signature, audience, issuer or nonce verification failed")?;
        let email = claims
            .email()
            .context("OIDC provider omitted verified email")?
            .as_str()
            .to_ascii_lowercase();
        if claims.email_verified() != Some(true) {
            bail!("OIDC email must be verified by the identity provider");
        }
        if !self.allowed_emails.contains(&email) {
            bail!("OIDC account is not in the explicit administrator allowlist");
        }
        Ok(VerifiedIdentity {
            email,
            issuer: self.issuer.clone(),
            subject: claims.subject().as_str().to_owned(),
        })
    }
}
