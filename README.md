# IndexLoom

IndexLoom is a self-hosted, extensible indexer manager and federated search
engine written in Rust with a React and TypeScript web interface.

> [!IMPORTANT]
> IndexLoom is in pre-alpha development. Its API, database and definition
> schema are not stable yet.

## Project goals

- A versioned, implementation-independent YAML definition format.
- Concurrent search with per-indexer isolation, timeouts and observability.
- Normalized categories, capabilities and results across heterogeneous sources.
- SQLite-first storage with a clean boundary for a future PostgreSQL backend.
- A documented REST API and a responsive web interface.
- Verifiable, licensed catalog conversion and explicit runtime import adapters.

## Repository boundaries

This repository contains only the original IndexLoom implementation and does
not vendor Jackett or Prowlarr source or definitions. The separate catalog
repository reproducibly converts compatible Cardigann definitions, pins their
source revisions, preserves their license and provenance, and removes duplicate
site definitions.

The generated and community-maintained IndexLoom catalog lives in the separate
[IndexLoom/Indexers](https://github.com/IndexLoom/Indexers) repository.

The product shell follows the design direction recorded in
[`docs/design`](docs/design/README.md). It uses real runtime and empty states;
the mockup's sample indexers and results are not shipped as application data.

The application owns the executable Rust contract for
`indexloom.io/v1alpha1`. Its JSON Schema is mirrored into the Indexers
repository and checked there in CI.

To validate a checked-out catalog against the Rust model:

```bash
cargo run -p indexloom-definitions --example validate_catalog -- \
  /path/to/Indexers/definitions
```

## Development

The supported development path is Docker Compose:

```bash
docker compose up --build
```

The API health endpoint is available at `http://127.0.0.1:8080/api/v1/health`.

## Versioning

IndexLoom follows Semantic Versioning. Application releases use Git tags such
as `v0.1.0`; REST API and definition schema versions evolve independently.

## License

IndexLoom is licensed under the GNU Affero General Public License v3.0 or later.
See [LICENSE](LICENSE).
