# Architecture

IndexLoom is a modular monolith. A single process owns the HTTP API, scheduler,
search orchestration and web assets while domain logic remains isolated in
workspace crates.

## Dependency direction

```text
indexloom-server
  -> indexloom-importers
  -> indexloom-engine
  -> indexloom-storage
  -> indexloom-definitions
  -> indexloom-core
```

Lower layers must not depend on the API or frontend. Third-party definition
formats are converted into the canonical IndexLoom model before validation or
execution.

## Persistence

SQLite in WAL mode is the initial supported database. Storage access is kept
behind repository traits so PostgreSQL can be added without leaking SQL dialect
details into the domain or API.

## Trust boundaries

Definitions and remote HTML are untrusted input. The engine will enforce URL
allowlists, private-network restrictions, response limits, parser limits,
timeouts and bounded concurrency before a definition can execute.

## Decisions and roadmap

Material choices are recorded as architecture decision records in
[`../decisions`](../decisions). The ordered delivery plan is in
[`../ROADMAP.md`](../ROADMAP.md).
