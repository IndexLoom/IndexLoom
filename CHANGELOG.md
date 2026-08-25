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

### Security

- Removed the shared HTTP cookie jar to prevent credentials crossing indexer
  boundaries.
- Added XChaCha20-Poly1305 authenticated encryption and redacted API responses
  for tracker secrets.
- Protected management endpoints with a constant-time bearer-token check and
  `no-store` responses.
