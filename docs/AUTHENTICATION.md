# IndexLoom authentication and email configuration

IndexLoom supports five **server-side** authentication modes. Authentication is
not just a frontend gate: all protected `/api/v1/*` endpoints share the same
authorization middleware. Existing installations stay in `token` mode unless
their environment explicitly opts into another policy.

## Supported modes

| `INDEXLOOM_AUTH_MODE` | Description | Required setup |
| --- | --- | --- |
| `token` (default) | Legacy administrator bearer token | `INDEXLOOM_ADMIN_TOKEN` of 32+ bytes |
| `local` | Named accounts using username/email and Argon2id password | First account requires independent administrator token **once** |
| `oidc` | Verified authorization-code + PKCE OIDC identities | HTTPS issuer, client and explicit email allowlist |
| `hybrid` | Operator token, local accounts and OIDC | Token and OIDC settings; accounts can be managed in Settings |
| `disabled` | Public, unauthenticated access | `INDEXLOOM_ALLOW_UNSAFE_NO_AUTH=true` confirmation; never expose publicly |

In local/hybrid modes, first open the app and create the first named
administrator using the operator token. Initial creation is a database-atomic
single-use operation, preventing a second bootstrap. You can later create more
local accounts from **Settings → Security & users**. Currently all enabled
users have administrator access; fine-grained role separation is **not**
implemented. Do not create untrusted accounts.

No token or plaintext password is stored in the account database. Password
hashes use salted Argon2id. Session, verification, password reset and OIDC state
tokens are indexed by SHA-256 digests. Session lifetimes are 12 hours or
7 days with "Remember me". Sessions are revocable; changing a password revokes
all sessions. Failed local login attempts are throttled by login identity.
The account owner can change their password with the current password or use
a one-time email reset link (when SMTP is enabled).

Existing `INDEXLOOM_ADMIN_TOKEN` is accepted only in token/hybrid mode for
normal protected endpoints; the standalone bootstrap endpoint also requires
it as independent proof of ownership. You must provision it securely.
Persisting a login token in the browser with "Remember me" uses localStorage:
avoid using this on shared/untrusted devices; for Internet-facing multi-user
deployments, cookie-based HTTP-only sessions are a planned hardening step.

## OIDC

The authorization code flow uses provider discovery over HTTPS, PKCE S256,
per-request CSRF state, OIDC nonce, verified JWT signature, trusted
issuer/audience and *verified email*. Subjects are permanently bound to an
issuer. **Accounts are never automatically linked by email** to prevent
provider identity collisions. Only emails in the explicit allowlist can
create or access OIDC accounts.

Set:

```dotenv
INDEXLOOM_AUTH_MODE=oidc
INDEXLOOM_OIDC_ISSUER=https://id.example.com
INDEXLOOM_OIDC_CLIENT_ID=indexloom
INDEXLOOM_OIDC_CLIENT_SECRET=replace-with-secret
INDEXLOOM_OIDC_ALLOWED_EMAILS=admin@example.org,other@example.org
INDEXLOOM_OIDC_REDIRECT_URI=https://indexloom.example.com/
```

Register `INDEXLOOM_OIDC_REDIRECT_URI` **exactly** at the provider. PKCE
works with clients without a secret, but confidential deployments should
supply a client secret. Both Pocket ID and other standards-compliant providers
can work once these requirements are met; no provider-specific login
integration is claimed without end-to-end testing.

Do not expose an open proxy endpoint as an OIDC issuer. HTTPS public URL
checks in application startup do not eliminate every DNS-rebinding scenario;
protect outbound networking at the infrastructure level.

## Optional email

SMTP is entirely optional. Disabled deployments continue to support local
login but cannot use email recovery, confirmation or notifications.
`INDEXLOOM_MAIL_MODE=disabled` is the default. For SMTP:

```dotenv
INDEXLOOM_MAIL_MODE=smtp
INDEXLOOM_PUBLIC_URL=https://indexloom.example.com
INDEXLOOM_SMTP_HOST=smtp.example.org
INDEXLOOM_SMTP_PORT=587
INDEXLOOM_SMTP_FROM=IndexLoom <indexloom@example.org>
INDEXLOOM_SMTP_USERNAME=indexloom@example.org
INDEXLOOM_SMTP_PASSWORD=replace-with-app-password
INDEXLOOM_REQUIRE_VERIFIED_EMAIL=false
```

Port 587 uses mandatory STARTTLS; 465 uses implicit TLS. No plaintext
transport mode is exposed. Set SMTP credentials through your secrets manager
or environment; never commit real credentials. Both
`INDEXLOOM_SMTP_PASSWORD_FILE` and `INDEXLOOM_OIDC_CLIENT_SECRET_FILE`
can read a secret from a regular file (maximum 64 KiB); use either the
direct environment value or its `_FILE` variant, not both. The sample
Compose file passes file paths but does not mount those files for you;
mount them read-only with appropriate permissions if using file secrets.

A configurable flag `INDEXLOOM_REQUIRE_VERIFIED_EMAIL=true` makes verified
email mandatory for locally authenticated users and requires SMTP.
The initial operator-proven bootstrap account is considered verified, to avoid
stranding the operator after SMTP misconfiguration.

Supported messages:

- Email verification — 24-hour, single-use confirmation.
- Password reset — 30-minute, single-use recovery.
- Login alert — optional notification of account sign-in.
- SMTP test — manual test to an administrator-chosen recipient.

The notification controls and four editable templates appear in **Settings →
Email & notifications**. Customize subject, plain-text body and optional HTML.
Available placeholders are `{{app}}`, `{{username}}`, `{{email}}`,
`{{action_url}}`, `{{event}}` and `{{time}}`. Placeholder values
in HTML are entity-escaped to prevent email injection via account names.
Raw admin-provided HTML is trusted by the server; only import templates from
trusted people. Never share a reset/verification URL: it is a bearer secret.

Template and notification settings are stored in SQLite; SMTP server settings
are startup environment configuration. Restart the container after changing
SMTP transport settings.

## Security boundaries and limitations

- Auth disabled is intentionally dangerous: every protected endpoint becomes
  publicly available. Restrict network exposure or leave authentication on.
- Token-only mode retains legacy behavior for old automation clients.
- Dynamic user roles, per-user permissions, WebAuthn/passkeys, TOTP,
  provider-specific social logins and full audit history are not yet implemented.
- Request rate limits use login identity; distributed attackers can rotate
  identifiers. Protect public deployments with a reverse proxy/WAF and TLS.
- Only static PWA resources are cached offline. Searching and changing accounts
  require a live server.
- SMTP and OIDC behavior must be integration-tested against the selected
  external identity provider and mail server before public production use.

## API endpoints

Public: `GET /api/v1/auth/config`, `POST /api/v1/auth/bootstrap`,
`/login`, `/oidc/start`, `/oidc/complete`, `/email/request-reset`,
`/email/reset`, `/email/verify`.

Protected: `GET /api/v1/auth/me`, `POST /api/v1/auth/logout`,
`/password`, `/email/request-verification`,
`GET /api/v1/auth/users`, `POST /api/v1/auth/users`,
`PUT /api/v1/auth/users/{id}/disabled`,
`POST /api/v1/auth/users/{id}/verify-email`,
`GET /api/v1/auth/mail`,
`PUT /api/v1/auth/mail/templates/{kind}`,
`PUT /api/v1/auth/mail/events/{kind}`,
`POST /api/v1/auth/mail/test`.

All protected endpoints accept `Authorization: Bearer <token>`; in
`local`/`oidc` modes it must be a server-issued access token.
