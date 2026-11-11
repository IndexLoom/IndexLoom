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

## UI and Cardigann evolution (November 2026)

- Implemented: six themed palettes with Obsidian Violet as default,
  editable token colors, layout, navigation, custom CSS and portable profiles.
- Implemented: catalog UI facets from native categories, languages, search
  modes and Jackett/Prowlarr provenance.
- Implemented: structured search variables for native categories, mode, genre,
  year, season/episode, IMDb and TMDb IDs, with explicit unsupported-mode errors.
- Implemented: bounded streaming for direct tracker responses and fallback to
  alternate search paths for transient transport/server errors.
- Pending: full Cardigann filter/authentication compatibility, tested real
  trackers, date/selector variants, Torznab/RSS, native source-specific plugins,
  dedicated full drag-and-drop layout designer and production security review.
- Security advisories in current SQLx's bundled SQLite and the YAML parser
  require upstream fixes or an independently tested parser/storage migration;
  routine Cargo.lock refresh does not automatically resolve them.

## Identity, mail and mobile PWA

Implementation under review in the feature branch:

- Local Argon2id sign-in, operator-token bootstrap, login throttling,
  separate revocable sessions, user management and account disabling.
- Verified OpenID Connect with PKCE, nonce, issuer/audience and a strict
  allowlist; hybrid and explicitly disabled authentication modes.
- Optional TLS SMTP; per-event email switches, customizable HTML/text templates,
  password recovery, verification and sign-in alerts.
- Mobile bottom navigation, tablet/laptop responsive layouts, iOS safe areas,
  installable PWA with static-only offline shell and PNG app icons.

Next milestones: verified real-provider integration and security review,
TOTP/WebAuthn/passkeys, recovery codes, per-indexer ACLs and extended audit logs,
push notifications, extended mail events, and the already planned Torznab,
RSS, tracker compatibility suite and persistent search history.

## Authentication hardening beyond the first MFA rollout

- [x] Local password login, verified OIDC and legacy/hybrid/disabled modes.
- [x] Owner/admin/viewer roles and account security event history.
- [x] Optional TOTP with HMAC-SHA256, backup codes, encrypted factors and replay protection.
- [ ] End-to-end tests against an external OIDC provider and SMTP server.
- [ ] HTTP-only SameSite session cookies and CSRF lifecycle for browser clients.
- [ ] FIDO2/WebAuthn passkeys, hardware authenticators and mandatory provider-independent MFA policies.
- [ ] Recovery workflows with supportable account-owner identity proof and secure audit exports.
- [ ] Replace or remove vulnerable upstream RustSec dependencies when compatible audited releases are available.
