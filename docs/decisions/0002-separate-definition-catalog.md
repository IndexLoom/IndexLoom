# ADR 0002: Maintain definitions in a separate repository

- Status: accepted
- Date: 2026-08-25

## Context

Application releases and indexer-site changes have different lifecycles.
Definition contributions also require specialized validation, provenance review,
and a smaller review surface than application code.

## Decision

Maintain the generated and community-authored IndexLoom YAML catalog in
`IndexLoom/Indexers`. Keep the canonical Rust model and execution engine in
`IndexLoom/IndexLoom`. Both repositories use the pre-stable
`indexloom.io/v1alpha1` contract and validate against matching JSON Schema
content.

Compatible Cardigann catalogs may be converted in the separate repository when
their licenses permit redistribution. Conversion must be deterministic and
lossless, pin exact upstream commits, preserve the source license and
attribution, record source and semantic hashes, and apply explicit
deduplication precedence. Upstream application source is not vendored.

The main repository may also expose runtime importers for user-supplied
definitions, but those importers are not the catalog update mechanism.

## Consequences

- Catalog fixes can be reviewed and released independently.
- A schema change requires coordinated pull requests across both repositories.
- Provenance and license review are part of definition contribution review.
- Generated definitions remain governed by their compatible source license;
  repository-authored tooling remains AGPL-3.0-or-later.
- The application must verify catalog signatures/checksums before automatic
  updates are introduced.
