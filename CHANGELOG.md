# Changelog

All notable changes to IndexLoom will be documented in this file.

The format is based on Keep a Changelog, and this project follows Semantic
Versioning.

## [Unreleased]

### Added

- Initial Rust workspace, React frontend and Docker development skeleton.
- Deterministic, provenance-aware Cardigann catalog model and validator.
- Authenticated indexer configuration API and React management interface.
- Per-indexer cookies, API credentials, login credentials, and `User-Agent`
  overrides with encrypted secret storage.
- Multiple named `FlareSolverr` connectors with per-indexer selection.
- Request-scoped direct HTTP profiles and a bounded `FlareSolverr` client.
- Verified startup and periodic synchronization of the separate Indexers
  catalog, preserving existing configuration and disabling new instances.
- Initial Cardigann execution for authenticated tests and concurrent grouped
  search with HTML and JSON result extraction.
- Named HTTP, SOCKS4a and SOCKS5h proxies with encrypted passwords and
  indexer-over-group-over-global precedence.
- Many-to-many indexer groups, tags, per-indexer enablement, bulk tests, search
  selection, persisted last-test state, and collapsible result groups.

### Security

- Removed the shared HTTP cookie jar to prevent credentials crossing indexer
  boundaries.
- Added XChaCha20-Poly1305 authenticated encryption and redacted API responses
  for tracker secrets.
- Protected management endpoints with a constant-time bearer-token check and
  `no-store` responses.
- Added catalog archive/extraction limits, manifest hash verification,
  same-origin redirect enforcement, response bounds, and bounded concurrency.
- Scoped proxy credentials and cookie jars to one indexer execution context.
