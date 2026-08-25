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

Direct HTTP requests do not share a cookie jar. Each request receives only the
credentials selected for that indexer. A configured `FlareSolverr` endpoint is
an administrator-controlled outbound trust boundary and should be reachable
only over a trusted local or private network.

Outbound proxy passwords are encrypted independently from tracker credentials
and are never returned by the API. SOCKS routes use remote DNS resolution to
avoid bypassing the selected route. A proxy administrator can still observe or
modify traffic according to the protocol's capabilities; only configure
trusted proxies and prefer end-to-end HTTPS tracker URLs.

Master-key rotation is not implemented yet. Back up the key independently from
the database and treat loss of the key as loss of the encrypted credentials.
