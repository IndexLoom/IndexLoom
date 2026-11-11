# Roadmap

The roadmap describes ordering, not release dates.

## Foundation (`0.1.x`)

- Stable project, container, CI, health API, and SQLite migrations.
- Canonical `v1alpha1` definition parser and validation.
- Safe outbound HTTP policy: SSRF controls, DNS rebinding resistance, timeouts,
  response limits, concurrency budgets, and redacted logs.
- Definition catalog fetch, integrity verification, and controlled updates.

## Search prototype (`0.2.x`)

- Indexer configuration and encrypted secret storage boundary.
- Concurrent search orchestration and normalized result categories.
- Search history, per-indexer health, structured errors, and API documentation.
- Execute and compatibility-test the generated Cardigann catalog from
  `IndexLoom/Indexers`.

The initial vertical slice is now present: verified catalog synchronization,
encrypted configuration, groups/tags/proxy routing, bounded bulk tests, and
grouped progressively streamed concurrent search, direct-DNS rebinding
resistance and bounded per-instance runtime/session reuse. Compatibility
coverage, normalized category maps, persisted search history, metrics, and API
stabilization remain before `0.2.x` is complete.

## Catalog distribution and packaging (`0.3.x`)

- Implemented: SQLite-first, independently published Indexers releases.
- Implemented: source-variant preservation across Jackett and Prowlarr.
- Implemented: release manifest comparison and verified runtime ingestion.
- Implemented: cross-platform web-enabled/headless Rust build workflows and
  multiarch Docker publishing.
- Pending: test real-world Cardigann behavior beyond static YAML validation,
  improve variant merging and category/genre/tag normalization, add a signed
  release/provenance chain and robust upgrade/rollback UX.

## Application integrations (`0.3.x`)

- Torznab-compatible API surface.
- Download-client integrations and notifications.
- Statistics, rate limiting, scheduling, and backup/restore.
- Compatibility-tested runtime import adapters where licensing permits.

## Stabilization (`0.x`)

- OIDC and local authentication with explicit threat modeling.
- PostgreSQL backend based on demonstrated demand.
- Definition schema promotion only after real catalog experience.
- Signed releases, SBOMs, provenance attestations, and upgrade documentation.
- Generate TypeScript API contracts from the Rust/OpenAPI source of truth.
- Advisory dependency audits for Rust and pnpm plus container vulnerability
  scanning, tuned separately from blocking license and source-policy checks.

## Snapshot retirement semantics

Catalog synchronization preserves retired definitions, all saved credentials, tags,
proxies and enabled preferences. A separate `catalogAvailable` state marks whether
an indexer exists in the current release. Retired instances are automatically excluded
from search and test selection, and resume with their previous settings if the ID
reappears. Empty catalog snapshots are rejected.

## Read-only catalog diagnostics

The protected `GET /api/v1/catalog/status` endpoint and Settings page show
available, retired, and total definitions, along with the last successfully
applied release timestamp. The refresh control only reloads diagnostics; it
does not bypass the configured background synchronization schedule.
