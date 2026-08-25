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
