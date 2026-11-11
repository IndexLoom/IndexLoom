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

For OIDC-only deployments, use a Compose variant which does **not** mount
any administrator token:

```bash
docker compose -f compose.no-operator.yaml up --build -d
```

Set `INDEXLOOM_AUTH_MODE=oidc` and the OIDC issuer/client/allowlist
variables in `.env` first; the first explicitly allowlisted OIDC account
becomes owner. The same file supports the intentionally dangerous public
mode only after BOTH `INDEXLOOM_AUTH_MODE=disabled` and
`INDEXLOOM_ALLOW_UNSAFE_NO_AUTH=true` are configured.
The default `compose.yaml` remains compatible with token-based deployments
and still requires the operator secret.

In local/hybrid modes, first open the app and create the first named
administrator using the operator token. Initial creation is a database-atomic
single-use operation, preventing a second bootstrap. You can later create more
local accounts from **Settings → Security & users**. Server-side access controls
distinguish three roles: **owner**, **admin** and **viewer**. Newly created local
users default to viewer, and later OIDC accounts start as viewer. The first
locally bootstrapped account or the first allowlisted OIDC identity becomes
owner. Existing accounts from older development migrations are retained and the
earliest one is promoted to owner.

Owner can manage users, email templates and security history; admin can
configure indexers and network routing but cannot manage identities or mail;
viewer can inspect indexers and perform searches without changing configuration.
Authorization happens for every API request, not only in the React UI. Last
active owner protection is enforced inside SQLite by an update/delete trigger.
The legacy operator token remains an owner-level credential in token/hybrid
mode. Disabling auth grants equivalent access to every requester.

Important: this is a coarse-grained initial RBAC model, not yet per-indexer
ownership, granular resource scopes or sharing permissions.

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

## Optional second factor for password login (TOTP)

Local and hybrid accounts with a password may enable time-based two-factor
authentication in **Settings → Security & users → Two-factor authentication**.

1. Sign in with the local username/email and password.
2. Re-enter the current password when beginning authenticator enrollment.
3. Use the displayed Base32 key or `otpauth://` link in a compatible
   authenticator application. IndexLoom uses **RFC 6238 SHA-256**, 6 digits,
   with 30-second codes and a ±30-second clock tolerance.
4. Confirm a valid current code to finish enrollment. The server then revokes
   every existing account session, including the setup session.
5. **Save the ten recovery codes** immediately. Each is 16 Base32 characters
   and works only once. Their values are shown only on the confirmation
   response, never available from the settings API afterward.

Subsequent local logins return a five-minute, one-use challenge rather than
an access token. Complete the second step with the authenticator code or a
recovery code. Each challenge allows at most five failures; account-level
failures are also rate-limited. A TOTP time-step cannot be accepted again
after successful verification, including across simultaneous sessions.
Only one outstanding password-verified challenge is kept per account.

The TOTP secret is encrypted with the existing
`INDEXLOOM_MASTER_KEY` using XChaCha20-Poly1305 and
account-specific authenticated associated data. Recovery codes and challenge
tokens are stored as SHA-256 hashes, not plaintext. Disabling a factor
requires both the current password and a current authenticator or recovery
code and revokes all sessions. The email-based password-reset flow does
**not** disable TOTP.

OIDC identity providers retain responsibility for their own MFA; the local
TOTP factor protects *local password logins*, not external OIDC sessions or
the legacy operator token. If an administrator needs mandatory MFA for
every access method, disable the operator-token route after bootstrap and
require MFA at the OIDC provider. A policy enforcing MFA across all auth
modes, passkeys/WebAuthn and hardware security keys remain future work.

If the master key is lost, encrypted TOTP secrets (as well as indexer
credentials) cannot be recovered. Use the originally displayed backup codes
or recover access through an appropriately secured operator procedure.

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
- Owner/admin/viewer authorization and a bounded 100-event security log are
  implemented; fine-grained indexer ACLs and event filtering/export are not.
  WebAuthn/passkeys, hardware security keys and single-sign-on MFA enforcement are not yet implemented.
- Request rate limits use login identity; distributed attackers can rotate
  identifiers. Protect public deployments with a reverse proxy/WAF and TLS.
- Only static PWA resources are cached offline. Searching and changing accounts
  require a live server.
- SMTP and OIDC behavior must be integration-tested against the selected
  external identity provider and mail server before public production use.

## API endpoints

Public: `GET /api/v1/auth/config`, `POST /api/v1/auth/bootstrap`,
`/login`, `/oidc/start`, `/oidc/complete`, `/totp/complete`, `/email/request-reset`,
`/email/reset`, `/email/verify`.

Protected: `GET /api/v1/auth/me`, `POST /api/v1/auth/logout`,
`/password`, `/email/request-verification`,
`GET /api/v1/auth/users`, `POST /api/v1/auth/users`,
`PUT /api/v1/auth/users/{id}/disabled`,
`PUT /api/v1/auth/users/{id}/role`,
`GET /api/v1/auth/audit`,
`POST /api/v1/auth/users/{id}/verify-email`,
`GET /api/v1/auth/totp`, `POST /api/v1/auth/totp/start`,
`POST /api/v1/auth/totp/confirm`, `POST /api/v1/auth/totp/disable`,
`GET /api/v1/auth/mail`,
`PUT /api/v1/auth/mail/templates/{kind}`,
`PUT /api/v1/auth/mail/events/{kind}`,
`POST /api/v1/auth/mail/test`.

All protected endpoints accept `Authorization: Bearer <token>`; in
`local`/`oidc` modes it must be a server-issued access token.
