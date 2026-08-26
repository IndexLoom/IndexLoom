# IndexLoom

[![CI](https://github.com/IndexLoom/IndexLoom/actions/workflows/ci.yml/badge.svg)](https://github.com/IndexLoom/IndexLoom/actions/workflows/ci.yml)
[![License: AGPL-3.0-or-later](https://img.shields.io/badge/license-AGPL--3.0--or--later-blue.svg)](LICENSE)
[![Rust 1.98](https://img.shields.io/badge/rust-1.98-orange.svg)](rust-toolchain.toml)

IndexLoom is a self-hosted indexer manager and federated torrent search engine
built with Rust, React and TypeScript. It combines a verifiable YAML catalog
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
- Store tracker cookies, API keys, passwords, passkeys and proxy credentials
  encrypted with XChaCha20-Poly1305.
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
| `INDEXLOOM_ADMIN_TOKEN` / `_FILE` | required | Independent API administrator credential |
| `INDEXLOOM_MASTER_KEY` / `_FILE` | required | Standard Base64 encoding of exactly 32 bytes |
| `INDEXLOOM_CATALOG_DIR` | empty | Optional local Indexers checkout |
| `INDEXLOOM_CATALOG_ARCHIVE_URL` | Indexers `main` archive | Remote catalog source |
| `INDEXLOOM_CATALOG_SYNC_SECONDS` | `21600` | Refresh period; `0` disables periodic refresh |
| `RUST_LOG` | `indexloom=info,tower_http=info` | Runtime log filter |

Do not reuse the admin token as the master key. Losing the master key makes
encrypted tracker credentials unrecoverable.

## Catalog lifecycle

Definitions live in the separate
[`IndexLoom/Indexers`](https://github.com/IndexLoom/Indexers) repository. On
startup IndexLoom downloads or reads the configured catalog, verifies its
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

The first Cardigann layer supports globally bounded concurrent tests and
searches, progressive POST/NDJSON search results, HTML/XML and JSON rows,
common templates and filters, entry-capped cookie/form sessions isolated by instance with idle expiry,
redirect and response-size limits, per-instance user agents, proxy routing and
optional FlareSolverr. Direct HTTP also enforces public-address DNS resolution
inside the connector to resist rebinding. Remote-DNS proxy and FlareSolverr
routes deliberately delegate address policy to their administrator-controlled
egress service. Unsupported definition behavior fails per indexer instead of
hiding successful results from other indexers.

See the [API guide](docs/API.md), [architecture notes](docs/architecture/README.md)
and [roadmap](docs/ROADMAP.md) for current limits and planned Torznab/RSS work.

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
- **IndexLoom/Indexers** — reproducible catalog conversion, provenance,
  schemas, validation and generated YAML definitions.

Jackett and Prowlarr source code is not vendored into either released
application artifact. Converted definitions retain their upstream license and
source metadata.

## Versioning and license

IndexLoom follows Semantic Versioning. Application tags such as `v0.3.0`, REST
API versions and definition schema versions evolve independently.

The application is licensed under
[GNU AGPL v3.0 or later](LICENSE). Generated catalog files retain the licenses
recorded by the Indexers repository.
