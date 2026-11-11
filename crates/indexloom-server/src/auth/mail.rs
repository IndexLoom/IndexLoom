//! Explicitly optional STARTTLS/SMTPS outbound email with configurable templates.
//! Only authenticated administrators can edit templates; no email content is cached.

#![allow(clippy::missing_errors_doc)]
#![allow(clippy::missing_fields_in_debug)]

use std::env;

use anyhow::{Context, Result, bail};
use indexloom_storage::{Store, auth::MailTemplate};
use lettre::{
    AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor,
    message::{Mailbox, MultiPart},
    transport::smtp::authentication::Credentials,
};

#[derive(Clone)]
pub struct MailSender {
    from: Mailbox,
    transport: AsyncSmtpTransport<Tokio1Executor>,
    public_url: String,
}

impl std::fmt::Debug for MailSender {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("MailSender")
            .field("public_url", &self.public_url)
            .field("credentials", &"<redacted>")
            .finish()
    }
}

#[derive(Debug)]
pub struct MailVariables<'a> {
    pub username: &'a str,
    pub email: &'a str,
    pub action_url: &'a str,
    pub event: &'a str,
}

pub const KINDS: [&str; 4] = ["verify-email", "reset-password", "login-alert", "test"];

pub fn default_template(kind: &str) -> MailTemplate {
    let (subject, body) = match kind {
        "verify-email" => (
            "Confirm your IndexLoom email",
            "Hello {{username}},\n\nConfirm your email address: {{action_url}}\n\nIf you did not request this, ignore this message.",
        ),
        "reset-password" => (
            "Reset your IndexLoom password",
            "Hello {{username}},\n\nReset your password using this one-time link: {{action_url}}\n\nThis link expires in 30 minutes. If you did not request it, ignore this message.",
        ),
        "login-alert" => (
            "New IndexLoom sign-in",
            "Hello {{username}},\n\nA new sign-in to your IndexLoom account was recorded at {{time}} (UTC).\n\nIf this was not you, change your password.",
        ),
        _ => (
            "IndexLoom email test",
            "Hello {{username}},\n\nYour IndexLoom SMTP configuration is working.\nSent at {{time}} UTC.",
        ),
    };
    MailTemplate {
        kind: kind.to_owned(),
        subject: subject.to_owned(),
        text_body: body.to_owned(),
        html_body: String::new(),
    }
}

pub fn render(source: &str, vars: &MailVariables<'_>, html: bool) -> String {
    let safe = |value: &str| {
        if html {
            value
                .replace('&', "&amp;")
                .replace('<', "&lt;")
                .replace('>', "&gt;")
                .replace('"', "&quot;")
                .replace('\'', "&#39;")
        } else {
            value.to_owned()
        }
    };
    let time = time::OffsetDateTime::now_utc().to_string();
    let replacements = [
        ("{{app}}", "IndexLoom".to_owned()),
        ("{{username}}", safe(vars.username)),
        ("{{email}}", safe(vars.email)),
        ("{{action_url}}", safe(vars.action_url)),
        ("{{event}}", safe(vars.event)),
        ("{{time}}", time),
    ];
    let mut text = source.to_owned();
    for (name, value) in replacements {
        text = text.replace(name, &value);
    }
    text
}

impl MailSender {
    /// SMTP is disabled unless explicitly configured. Invalid partial configs fail startup.
    pub fn from_env() -> Result<Option<Self>> {
        let enabled = env::var("INDEXLOOM_MAIL_MODE").unwrap_or_else(|_| "disabled".to_owned());
        if enabled == "disabled" {
            return Ok(None);
        }
        if enabled != "smtp" {
            bail!("INDEXLOOM_MAIL_MODE must be disabled or smtp");
        }

        let host = env::var("INDEXLOOM_SMTP_HOST").context("INDEXLOOM_SMTP_HOST is required")?;
        if host.is_empty() {
            bail!("SMTP hostname is empty");
        }
        let port = env::var("INDEXLOOM_SMTP_PORT")
            .unwrap_or_else(|_| "587".to_owned())
            .parse::<u16>()
            .context("INDEXLOOM_SMTP_PORT must be a port")?;
        let from: Mailbox = env::var("INDEXLOOM_SMTP_FROM")
            .context("INDEXLOOM_SMTP_FROM is required")?
            .parse()
            .context("INDEXLOOM_SMTP_FROM is invalid")?;
        let public_url = env::var("INDEXLOOM_PUBLIC_URL")
            .context("INDEXLOOM_PUBLIC_URL is required for SMTP links")?;
        let url = url::Url::parse(&public_url).context("INDEXLOOM_PUBLIC_URL is invalid")?;
        if url.scheme() != "https" && !matches!(url.host_str(), Some("localhost" | "127.0.0.1")) {
            bail!("INDEXLOOM_PUBLIC_URL must be HTTPS outside localhost");
        }
        if url.query().is_some()
            || url.fragment().is_some()
            || !url.username().is_empty()
            || url.password().is_some()
        {
            bail!("INDEXLOOM_PUBLIC_URL must not contain credentials, query or fragment");
        }
        let mut transport = if port == 465 {
            AsyncSmtpTransport::<Tokio1Executor>::relay(&host)
                .context("SMTP implicit TLS relay configuration failed")?
        } else {
            AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(&host)
                .context("SMTP STARTTLS configuration failed")?
        }
        .port(port);
        let username = env::var("INDEXLOOM_SMTP_USERNAME").unwrap_or_default();
        let password = env::var("INDEXLOOM_SMTP_PASSWORD").unwrap_or_default();
        if !username.is_empty() || !password.is_empty() {
            if username.is_empty() || password.is_empty() {
                bail!("SMTP username and password must both be supplied");
            }
            transport = transport.credentials(Credentials::new(username, password));
        }
        Ok(Some(Self {
            transport: transport.build(),
            from,
            public_url: public_url.trim_end_matches('/').to_owned(),
        }))
    }

    #[must_use]
    pub fn action_url(&self, path: &str, token: &str) -> String {
        format!("{}/?{}={}", self.public_url, path, token)
    }

    pub async fn send(
        &self,
        store: &Store,
        kind: &str,
        recipient: &str,
        vars: MailVariables<'_>,
    ) -> Result<()> {
        let template = store
            .mail_template(kind)
            .await?
            .unwrap_or_else(|| default_template(kind));
        let subject = render(&template.subject, &vars, false);
        if subject.chars().any(char::is_control) {
            bail!("email subject contains control characters");
        }
        let text = render(&template.text_body, &vars, false);
        let message = Message::builder()
            .from(self.from.clone())
            .to(recipient.parse().context("invalid destination email")?)
            .subject(subject);
        let message = if template.html_body.is_empty() {
            message.body(text).context("failed to build email")?
        } else {
            message
                .multipart(MultiPart::alternative_plain_html(
                    text,
                    render(&template.html_body, &vars, true),
                ))
                .context("failed to build multipart email")?
        };
        self.transport
            .send(message)
            .await
            .context("SMTP transport failure")?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{MailVariables, render};
    #[test]
    fn escapes_placeholder_values_in_html_and_preserves_custom_markup() {
        let vars = MailVariables {
            username: "<script>bad</script>",
            email: "user@example.org",
            action_url: "https://example.org/verify?a=1&b=2",
            event: "verify",
        };
        let result = render(
            "<strong>{{username}}</strong><a href=\"{{action_url}}\">Verify</a>",
            &vars,
            true,
        );
        assert!(result.contains("&lt;script&gt;"));
        assert!(!result.contains("<script>"));
        assert!(result.contains("a=1&amp;b=2"));
    }
}
