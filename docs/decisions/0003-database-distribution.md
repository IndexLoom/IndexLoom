# ADR 0003: Publish the Indexers catalog as an independent SQLite database

- Status: accepted
- Date: 2026-11-11
- Supersedes the source-storage portion of ADR 0002

## Context

The former Indexers repository committed hundreds of generated YAML files and
a large manifest. This tied application code review to catalog contents,
hid upstream differences whenever the same ID appeared in both Jackett and
Prowlarr, and gave no independent, versioned binary distribution.

## Decision

The source repository contains only importers, schema, validation tooling and
documentation. A scheduled GitHub Actions release workflow fetches exact
upstream revisions and generates a complete SQLite database and compressed
exports. Generated YAML exists only as temporary CI build input.

Every canonical indexer retains its lossless Cardigann YAML and searchable
semantic JSON. The database also retains every original Jackett and Prowlarr
source variant, including duplicates and differences not selected as the
canonical execution definition. Indexed tables expose categories, settings,
modes, URLs, capabilities, provenance and deduplication aliases.

Distribution assets are `indexers.db`, `indexers.db.zst`,
`indexers.jsonl.zst`, `indexers.tsv.zst`, a gzip JSONL transport for the
application, `manifest.json` and `SHA256SUMS`.

The application checks the latest release manifest on startup and at the
configured interval. It verifies the gzip export's size, SHA-256, record count,
individual definition checksums and schema before importing in one SQLite
transaction. The last accepted manifest fingerprint is persisted to avoid
re-importing an unchanged release.

## Consequences

- Application binaries contain no indexer catalog and release independently.
- A catalog update can be published without recompiling or restarting IndexLoom.
- Legacy local YAML checkout/archive loading remains temporarily available
  while users migrate to the release feed.
- Source differences remain preserved even though automatic selection still
  follows the legacy Jackett-first precedence rule.
- Future format changes must version both the distribution manifest and the
  runtime importer. Data compatibility is independent of application versions.
- SHA-256 protects the fetched asset against accidental corruption relative to
  its manifest; it does **not** provide an independent digital signature.
