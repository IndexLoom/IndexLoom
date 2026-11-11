# Security Policy

IndexLoom is currently pre-alpha and does not yet provide stable releases.

Please do not disclose vulnerabilities in public issues. Use GitHub private
vulnerability reporting for the IndexLoom repository. Include affected
versions, configuration, impact and reproduction steps without production
credentials or personal data.

Security-sensitive areas include definition execution, SSRF boundaries,
credential storage, parser resource limits, authentication and outbound proxy
handling.

## Credential model

Configuration writes require `INDEXLOOM_ADMIN_TOKEN`. Tracker passwords,
cookies, API keys, passkeys, PINs, and 2FA tokens are encrypted with
XChaCha20-Poly1305 under the independent `INDEXLOOM_MASTER_KEY`. The API returns
only the names of configured secret fields, never their values. The master key
must be supplied as standard Base64 for exactly 32 bytes and kept outside the
database and source tree.

For production, prefer `INDEXLOOM_ADMIN_TOKEN_FILE` and
`INDEXLOOM_MASTER_KEY_FILE` with Docker or Kubernetes secret mounts over direct
environment values. A direct variable and its `_FILE` counterpart are mutually
exclusive. The loader follows platform-managed symlinks, requires a regular
UTF-8 file no larger than 64 KiB and removes only one trailing LF or CRLF.
Secrets are read once during startup, so rotating a mounted file requires a
controlled application restart. Errors never include the secret value or its
configured filesystem path.

Direct HTTP clients are isolated by configured indexer instance. Declared URL
origins are enforced on the initial request and every redirect; IP literals are
rejected unless publicly routable. Direct hostname resolution runs inside the
reqwest connector, rejects mixed public/private answers, and returns only the
validated addresses that may be used for the connection. Environment proxy
variables are disabled so they cannot bypass the selected route.

An active instance runtime and its cookie jar are capped by entry count and are
evicted after 30 minutes without use.
Successful form/cookie authentication is reused for at most 15 minutes and is
serialized per instance. The runtime fingerprint covers definition content,
credentials, user agent, effective proxy and FlareSolverr configuration, so a
changed value creates a new isolated runtime for subsequent requests.

A configured HTTP/SOCKS proxy or `FlareSolverr` endpoint is an
administrator-controlled outbound trust boundary. HTTP proxies, SOCKS4a,
SOCKS5h and the FlareSolverr browser perform target DNS outside IndexLoom, so
local network-address confinement cannot be claimed for those routes. Enforce
equivalent public-only egress policy at the trusted proxy/connector and keep it
on a controlled network.

Outbound proxy passwords are encrypted independently from tracker credentials
and are never returned by the API. SOCKS routes use remote DNS resolution to
avoid bypassing the selected route. A proxy administrator can still observe or
modify traffic according to the protocol's capabilities; only configure
trusted proxies and prefer end-to-end HTTPS tracker URLs.

Master-key rotation is not implemented yet. Back up the key independently from
the database and treat loss of the key as loss of the encrypted credentials.

## Catalog update trust boundary

A configured catalog URL must use HTTPS and a non-local DNS hostname; redirects
must also stay on HTTPS hostnames. Source metadata and release assets are checked
against their recorded sizes and SHA-256 digests before a catalog transaction is
committed. Manifest downloads are limited to 1 MiB and exported catalogs to
64 MiB compressed / 96 MiB extracted. SHA-256 detects corrupted artifacts but
is **not** a substitute for signed, independently authenticated releases.
Administrators should only configure trusted release feeds, and use network egress
policies where DNS rebinding protection is required.
