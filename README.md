# IndexLoom

[![CI](https://github.com/IndexLoom/IndexLoom/actions/workflows/ci.yml/badge.svg)](https://github.com/IndexLoom/IndexLoom/actions/workflows/ci.yml)
[![License: AGPL-3.0-or-later](https://img.shields.io/badge/license-AGPL--3.0--or--later-blue.svg)](LICENSE)
[![Rust 1.98](https://img.shields.io/badge/rust-1.98-orange.svg)](rust-toolchain.toml)

IndexLoom is a self-hosted indexer manager and federated torrent search engine
built with Rust, React and TypeScript. It combines a separately released SQLite catalog
with isolated tracker instances, encrypted credentials, grouped search and
explicit HTTP, SOCKS and FlareSolverr routing.

> [!IMPORTANT]
> IndexLoom is pre-alpha. APIs, database migrations and the definition schema
> may change before the first stable release. New catalog entries are disabled
> by default and the current Cardigann interpreter intentionally implements a
> bounded subset rather than claiming drop-in Jackett compatibility.

## Why IndexLoom

- Configure multiple instances of one definition, such as a direct RuTracker
  instance and another routed through a proxy.
- Search all enabled instances, selected instances or reusable groups while
  preserving per-indexer result and error boundaries.
- Use structured filters (mode, category, genre, year, season, episode, IMDb
  and TMDb IDs) with tracker-specific category conversion and explicit errors
  for unsupported modes/categories.
- Explore imported Jackett/Prowlarr categories, modes and origin metadata and
  filter the indexer catalog by language, source, category or mode.
- Personalize the browser workspace with a dark violet default, six
  palettes, custom colors, CSS, sidebar position, density, panel split and
  reorderable/hideable workspace blocks, plus JSON profile import/export.
- Store tracker cookies, API keys, passwords, passkeys and proxy credentials
  encrypted with XChaCha20-Poly1305.
- Select legacy bearer-token, local username/email + Argon2id password, verified
  OpenID Connect (PKCE) or hybrid authentication; explicit insecure no-auth
  mode is available only with a separate operator acknowledgement.
- Enforce owner/admin/viewer API roles with least-privileged new accounts,
  database protection against losing the final owner, and a security event log.
- Optionally configure SMTP with per-event notification switches, editable
  plain-text/HTML templates, email verification and password reset links.
- Install a responsive PWA on phones, tablets or laptops with a mobile bottom
  bar, offline static shell, safe-area support and device-local appearance.
- Route globally, by group or by indexer through HTTP, SOCKS4a or SOCKS5h, with
  optional FlareSolverr for definitions that need browser challenge handling.
- Synchronize a separate, reproducible catalog transactionally while retaining
  the last valid snapshot when an update fails.
- Keep the execution engine, storage, API, frontend and catalog tooling behind
  explicit crate and repository boundaries.

## Architecture

```text
React + TypeScript UI
          │
       Axum API
          │
 ┌────────┼───────────┐
 │        │           │
SQLite  Cardigann   Catalog sync
store   executor         │
 │        │          IndexLoom/Indexers
 │        ├── direct HTTP
 │        ├── HTTP / SOCKS proxy
 │        └── FlareSolverr
 └── encrypted configuration and test history
```

The application is a modular Rust workspace. SQLite is the first supported
backend; storage boundaries are designed so a PostgreSQL backend can be added
without coupling definition execution to SQL.

## Quick start

Requirements: Docker Engine with Compose v2.

```bash
cp .env.example .env
openssl rand -hex 32
openssl rand -base64 32
```

Put the independent values in `INDEXLOOM_ADMIN_TOKEN` and
`INDEXLOOM_MASTER_KEY`. The bundled Compose file presents them to the
non-root process as read-only files under `/run/secrets`, then starts the
loopback-bound development deployment:

```bash
docker compose up --build -d
curl --fail http://127.0.0.1:8080/api/v1/health
```

Open `http://127.0.0.1:8080`, enter the administrator token for the current
browser tab, and configure only the indexers you intend to use. A reverse proxy
can be added separately when remote access is required.

The application also accepts `INDEXLOOM_ADMIN_TOKEN_FILE` and
`INDEXLOOM_MASTER_KEY_FILE` in custom Docker or Kubernetes deployments. Each
pair is mutually exclusive. Mounted secret files may end in one LF or CRLF;
other content is preserved and validated normally. A bind-mounted host file
must be readable by container UID/GID `10001`; platform-managed secret symlinks
are supported.

| Variable | Default | Purpose |
| --- | --- | --- |
| `INDEXLOOM_BIND` | `0.0.0.0:8080` | HTTP listener inside the container |
| `INDEXLOOM_DATABASE_URL` | `sqlite:///data/indexloom.db?mode=rwc` | Storage connection string |
| `INDEXLOOM_ADMIN_TOKEN` / `_FILE` | required in token/hybrid and initial local bootstrap | Independent operator credential; not accepted for ordinary API access in local/OIDC mode |
| `INDEXLOOM_MASTER_KEY` / `_FILE` | required | Standard Base64 encoding of exactly 32 bytes |
| `INDEXLOOM_AUTH_MODE` | `token` | `token`, `local`, `oidc`, `hybrid` or `disabled` |
| `INDEXLOOM_MAIL_MODE` | `disabled` | Optional SMTP; keep disabled if notifications and recovery are unnecessary |
| `INDEXLOOM_REQUIRE_VERIFIED_EMAIL` | `false` | Require verified local email addresses when SMTP is enabled |
| `INDEXLOOM_ALLOW_UNSAFE_NO_AUTH` | `false` | Must be `true` before selecting `disabled`; never use on publicly accessible servers |
| `INDEXLOOM_CATALOG_DIR` | empty | Optional local Indexers checkout |
| `INDEXLOOM_CATALOG_SOURCE_URL` | Latest Indexers release manifest | Release source URL (legacy archives also supported) |
| `INDEXLOOM_CATALOG_SYNC_SECONDS` | `21600` | Refresh period; `0` disables periodic refresh |
| `RUST_LOG` | `indexloom=info,tower_http=info` | Runtime log filter |

Do not reuse the admin token as the master key. Losing the master key makes
encrypted tracker credentials unrecoverable.

## Authentication, email and PWA

The default token-only setup is unchanged and remains valid for existing
deployments. **Settings → Security & users** manages local accounts, password
changes and access mode status. **Settings → Email & notifications** manages
notification switches and editable message templates. Auth and SMTP
configuration are server-side startup settings; passwords and SMTP credentials
must not be committed to this repository.

For local login, configure `INDEXLOOM_AUTH_MODE=local` and use the operator
token once to bootstrap the initial account. OIDC requires a verified HTTPS
issuer, PKCE-compatible client and explicit permitted email list. Switching
to `disabled` disables authorization of all protected API routes; do this
only on a trusted, isolated network.

Detailed [authentication and SMTP guide](docs/AUTHENTICATION.md), including
OIDC setup, security limitations, recovery, and all API endpoints.
[PWA and responsive installation guide](docs/PWA.md) covers smartphones,
tablets, standalone installs and offline behavior.

## Catalog lifecycle

Definitions live in the separate
[`IndexLoom/Indexers`](https://github.com/IndexLoom/Indexers) repository. On
startup IndexLoom downloads the latest release manifest (or reads an optional local legacy checkout), verifies its
manifest, source metadata, schema, IDs and SHA-256 digests, then updates SQLite
in one transaction. Existing instance configuration is retained; imported
definitions never become active automatically.

The runtime contract is `indexloom.io/v1alpha1`. Validate a checkout against
the Rust model with:

```bash
cargo run -p indexloom-definitions --example validate_catalog -- \
  /path/to/Indexers/definitions
```

## Result identity and links

Search results deliberately keep configuration identity separate from catalog
identity:

- `id` is an ephemeral UUID for one result in one search response.
- `indexerInstanceId` identifies the configured instance that performed the
  request.
- `definitionId` identifies its catalog definition.
- `sourceId`, `sourceGuid` and `infoHash` carry tracker-native stable identity
  when the definition exposes it.
- `detailsUrl`, `downloadUrl` and `magnetUrl` remain separate because a release
  page is not interchangeable with a `.torrent` endpoint or magnet URI.

This model supports future deduplication, RSS history, Torznab GUIDs and
duplicate-grab prevention without treating a UI row UUID as persistent state.

## Current execution scope

The evolving Cardigann layer supports globally bounded concurrent tests and
searches, progressive POST/NDJSON search results, HTML/XML and JSON rows,
common templates and filters, entry-capped cookie/form sessions isolated by instance with idle expiry,
redirect and per-chunk response-size limits, retryable multi-path searches,
per-instance user agents, proxy routing and
optional FlareSolverr. Direct HTTP also enforces public-address DNS resolution
inside the connector to resist rebinding. Remote-DNS proxy and FlareSolverr
routes deliberately delegate address policy to their administrator-controlled
egress service. Unsupported definition behavior fails per indexer instead of
hiding successful results from other indexers.

See the [API guide](docs/API.md), [architecture notes](docs/architecture/README.md)
and [roadmap](docs/ROADMAP.md) for current limits and planned Torznab/RSS work.

## Interface customization

Open **Settings → Appearance** to choose Obsidian Violet (the black/purple
default), Carbon, Midnight Blue, Evergreen, Burgundy or light Porcelain.
Change the exact color values, sidebar location, density, corner radius,
typography scale, indexer panel width and individual page block visibility or
order. Custom CSS is appended after the built-in stylesheet and can override
any part of the application. Appearance profiles can be exported/imported as
JSON, validated on import and stored in this browser's local storage. They do
not change server configuration or indexer credentials.

See [UI customization guide](docs/design/APPEARANCE.md) for CSS examples,
selectors, limitations and data retention.

## Binary releases

Application releases are built automatically for Windows x86_64, Linux x86_64
and ARM64, macOS Intel and Apple Silicon. Each platform archive includes two
separate Rust executables: `indexloom-web` embeds the compiled React UI and
serves it locally, while `indexloom-headless` exposes only the HTTP API.
No Jackett, Prowlarr or Node.js runtime is required. The browser UI is not a
native desktop window.

Container images are published for amd64 and arm64 to
`ghcr.io/indexloom/indexloom:edge` and a versioned pre-release tag.
The standalone binaries and Docker images require the same
`INDEXLOOM_ADMIN_TOKEN`, `INDEXLOOM_MASTER_KEY`, and writable SQLite
database configuration. Release downloads live in GitHub Releases.

## Development

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --all-targets --locked

cd web
pnpm install --frozen-lockfile
pnpm typecheck
pnpm build
```

CI also enforces dependency source, license and duplicate-version policy with
`cargo-deny`. Security advisories are reported without allowing a newly
published advisory to make every unrelated contribution unmergeable.

Before contributing, read [CONTRIBUTING.md](CONTRIBUTING.md) and
[SECURITY.md](SECURITY.md). TypeScript API models are currently checked
manually; generating them from the Rust/OpenAPI contract is tracked as a
pre-stable hardening task.

## Repository boundaries

- **IndexLoom/IndexLoom** — original application, API, UI, database migrations
  and execution engine.
- **IndexLoom/Indexers** — reproducible catalog import, provenance,
  schemas, validation and independently released SQLite databases.

Jackett and Prowlarr source code is not vendored into either released
application artifact. Converted definitions retain their upstream license and
source metadata.

## Versioning and license

IndexLoom follows Semantic Versioning. Application tags such as `v0.3.0`, REST
API versions and definition schema versions evolve independently.

The application is licensed under
[GNU AGPL v3.0 or later](LICENSE). Generated catalog files retain the licenses
recorded by the Indexers repository.
