# Authentication, identity and email

IndexLoom has **five explicit startup authentication modes**. New deployments
continue to use the original administrator bearer token until the operator
chooses a different mode. The master key remains independently required to
decrypt indexer secrets.

| Mode | What is accepted | Recommended context |
| --- | --- | --- |
| `token` (default) | `INDEXLOOM_ADMIN_TOKEN` | Existing installations and CLI |
| `local` | Managed username/email + Argon2id password | Single-host interactive UI |
| `oidc` | Verified OpenID Connect identity with allowlisted emails | SSO |
| `hybrid` | Local, verified OIDC, and operator token | Migration between login methods |
| `disabled` | No authentication | Isolated experiments only |

`disabled` requires **both**
`INDEXLOOM_AUTH_MODE=disabled` and
`INDEXLOOM_ALLOW_UNSAFE_NO_AUTH=true`. Every API endpoint becomes
accessible without credentials. Never use this mode on the public Internet,
even behind an unprotected reverse proxy.

## Local account first-run setup

Set `INDEXLOOM_AUTH_MODE=local` (or `hybrid`) and retain the original
`INDEXLOOM_ADMIN_TOKEN` for bootstrapping. Open the PWA / browser, enter
a username (3–48 ASCII letters/digits/`_`/`-`), a valid email, a password of
at least 12 bytes, and the independent operator token. The backend permits
**one** bootstrap insertion; competing requests cannot initialize multiple
owners. In `local` mode the bootstrap token is **not accepted by management
endpoints**. It is accepted as a break-glass token only in `hybrid`.

Administrators can create and disable further local users in **Settings >
Security & users**. All accounts currently have full administrator privileges:
there is no viewer/guest RBAC yet. The final enabled account cannot be
disabled. Disabling a user revokes their sessions.

The password algorithm is **Argon2id** with cryptographically random
per-password 128-bit salt and a 256-bit derived key. Only the encoded result
is persisted. Password changes and recovery invalidate all existing sessions.
Bearer session secrets are 256-bit random, indexed in SQLite by SHA-256 hashes,
and have either a 12-hour lifetime or 7 days when "Keep me signed in" is
explicitly selected. On the web the short session is stored in tab
`sessionStorage`; persistent sessions use `localStorage` and carry normal
browser/XSS risks. Never share profiles or install untrusted CSS.

Local login is throttled by normalized account identity: five failed attempts
within fifteen minutes trigger a fifteen-minute lockout. Rate limits and
session state survive restarts. For Internet deployments also configure
IP-aware rate limiting at the trusted reverse proxy.

## OpenID Connect

Configure an OpenID Connect provider:

```env
INDEXLOOM_AUTH_MODE=oidc
INDEXLOOM_OIDC_ISSUER=https://id.example.org
INDEXLOOM_OIDC_CLIENT_ID=indexloom
INDEXLOOM_OIDC_CLIENT_SECRET=REPLACE_WITH_SECRET
INDEXLOOM_OIDC_ALLOWED_EMAILS=admin@example.org,other@example.org
INDEXLOOM_OIDC_REDIRECT_URI=https://indexloom.example.org/
```

The server uses HTTPS discovery, Authorization Code Flow, SHA-256 PKCE,
one-time server-side state, nonce, JWKS-backed ID token validation (issuer,
audience, signature, expiry and nonce) and explicitly requires
`email_verified=true` from the identity provider. A fresh verified identity
is created only when its email matches the exact operator allowlist.
Existing local identities are **not linked by email**; this prevents an
unintended account takeover. Tokens and provider credentials are never put
into outgoing login-link emails.

OIDC requires a publicly reachable HTTPS callback URL (localhost exceptions
exist for local testing). Use a provider such as Pocket ID, Authentik,
Keycloak or another standards-compliant OIDC issuer. Validate the actual
provider configuration and account claims before production use.

## Email and notifications

Email is independently disabled by default. To enable SMTP with required
STARTTLS (port 587) or implicit TLS (port 465):

```env
INDEXLOOM_MAIL_MODE=smtp
INDEXLOOM_PUBLIC_URL=https://indexloom.example.org
INDEXLOOM_SMTP_HOST=smtp.example.org
INDEXLOOM_SMTP_PORT=587
INDEXLOOM_SMTP_FROM=IndexLoom <noreply@example.org>
INDEXLOOM_SMTP_USERNAME=noreply@example.org
INDEXLOOM_SMTP_PASSWORD=REPLACE_WITH_SECRET
INDEXLOOM_REQUIRE_VERIFIED_EMAIL=true
```

**Settings > Email & notifications** provides per-event toggles for
verification, password recovery and login alerts, editable plaintext and
HTML templates for each event plus SMTP test messages. Templates support
`{{app}}`, `{{username}}`, `{{email}}`, `{{action_url}}`,
`{{event}}`, and `{{time}}`; placeholder values are HTML-escaped when
rendered into HTML bodies. Email is sent only when configured and enabled.

Password-reset links expire in **30 minutes**; email verification links in
**24 hours**. Both use single-use 256-bit tokens, never stored in plaintext.
Self-registration is intentionally disabled; an existing administrator must
create users or explicitly allow an OIDC email. Email verification of the
initial bootstrap account is bypassed to prevent a broken mail server
permanently locking out the operator holding the bootstrap secret.

No mail service is needed for token or OIDC-only operation. When mail is off,
password resets by email and email notifications are unavailable; an operator
must use another configured account/recovery route.

## Migration and operating notes

1. Back up the existing SQLite database and master key.
2. Upgrade while leaving `INDEXLOOM_AUTH_MODE=token` to preserve old behavior.
3. Configure and test SMTP or OIDC independently.
4. Set `local`, `oidc` or `hybrid` and verify login before removing legacy
   token fallback.
5. For reverse proxies, terminate TLS, do not cache `/api/`, do not log
   Authorization headers or one-time reset/callback query parameters.

The auth policy is configured **at startup** by environment variables.
The Settings page displays the active policy, but does not allow a browser
session to silently disable its own protection. This is intentional.
Full RBAC, TOTP/WebAuthn, passkeys, scheduled digests and richer audit events
are planned, not shipped in this phase.
