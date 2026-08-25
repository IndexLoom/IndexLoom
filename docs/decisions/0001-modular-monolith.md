# ADR 0001: Start as a modular monolith

- Status: accepted
- Date: 2026-08-25

## Context

IndexLoom needs an HTTP API, scheduler, search execution, persistence, importers,
and a web interface. Splitting those concerns into network services before their
contracts stabilize would add deployment and compatibility cost without giving
the early project useful isolation.

## Decision

Ship one application process and keep the internal dependency boundaries in
Rust workspace crates. SQLite with WAL is the first persistence backend. The
storage crate must prevent SQL details from spreading into the domain and API so
a PostgreSQL implementation can be introduced later.

## Consequences

- A complete installation is one application container and one data volume.
- Internal modules can be tested separately without distributed coordination.
- A service may be extracted only after a measured scaling or isolation need.
- Database and API boundaries still require explicit versioning.
