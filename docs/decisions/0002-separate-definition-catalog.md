# ADR 0002: Maintain definitions in a separate repository

- Status: accepted
- Date: 2026-08-25

## Context

Application releases and indexer-site changes have different lifecycles.
Definition contributions also require specialized validation, provenance review,
and a smaller review surface than application code.

## Decision

Maintain original IndexLoom YAML definitions in `IndexLoom/Indexers`. Keep the
canonical Rust model and execution engine in `IndexLoom/IndexLoom`. Both
repositories use the pre-stable `indexloom.io/v1alpha1` contract and validate
against matching JSON Schema content.

Jackett, Prowlarr, and other projects may be studied as external references.
Their source and definitions are not vendored or mechanically copied. Explicit
runtime importers may convert user-provided external definitions when their
license and format permit it.

## Consequences

- Catalog fixes can be reviewed and released independently.
- A schema change requires coordinated pull requests across both repositories.
- Provenance and license review are part of definition contribution review.
- The application must verify catalog signatures/checksums before automatic
  updates are introduced.
