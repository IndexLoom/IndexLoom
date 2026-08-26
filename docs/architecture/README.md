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

Indexer non-secret values are stored as JSON. Passwords, cookies, API keys, and
other secret fields are stored in a separate table as one authenticated
XChaCha20-Poly1305 envelope per indexer. The indexer ID and format version are
authenticated as associated data. API models expose only configured secret
names, while an execution-only storage model performs decryption.

## Trust boundaries

Definitions and remote HTML are untrusted input. Catalog archives have bounded
download/extraction sizes and every manifest entry is verified before an
atomic SQLite update. The executor restricts requests and redirects to declared
tracker origins, bounds response bodies, applies timeouts, and uses bounded
concurrency. Newly discovered indexers remain disabled until an administrator
opts in.

Management endpoints require an explicit bearer token and return `no-store`.
Direct tracker HTTP has no cross-instance cookie jar. Its connector-level DNS
resolver rejects non-public and mixed answers before returning the exact
addresses reqwest may connect to; IP literals and every redirect are checked as
well. Per-instance runtimes have bounded count and idle lifetime and are replaced whenever
their configuration fingerprint changes.

`FlareSolverr` is configured as a named administrator-controlled connector and
selected per indexer; the target URL must still pass the declared-origin policy
before solver execution. Target address confinement is delegated to the
connector for FlareSolverr and to the proxy for remote-DNS HTTP/SOCKS routes.

Outbound routes are named HTTP, SOCKS4a or SOCKS5h records. Passwords use a
separate authenticated-encryption context from tracker credentials. Resolution
is deterministic: an indexer override wins, otherwise the enabled member group
with the highest priority wins, otherwise the global route is used. Equal group
priorities are broken by stable group ID ordering.

Search has a compatibility JSON response and a POST NDJSON stream. The stream
emits one indexer batch as each concurrent worker finishes, carries the bearer
token in the normal request header, and aborts outstanding `JoinSet` tasks when
the client disconnects. Search concurrency is global to the process rather
than multiplied per incoming search request. Test-job progress is retained for
24 hours with a 256-completed-job cap and an eight-active-job admission limit.

## Cardigann compatibility stage

The current interpreter is intentionally an executable compatibility layer,
not a claim of complete Jackett parity. It supports the first declared search
path, Go-style templates used by the converted catalog, common text filters,
GET/POST/form authentication, configured cookies, HTML and JSON extraction,
and optional FlareSolverr GET execution. Interactive CAPTCHA acquisition,
advanced selector-input workflows, every legacy filter, multi-path fallback,
full XML semantics, and broad date parsing remain explicit compatibility work.
An unsupported construct fails that indexer with a structured error; it does
not weaken the outbound policy or abort other indexers in the same search.

## Decisions and roadmap

Material choices are recorded as architecture decision records in
[`../decisions`](../decisions). The ordered delivery plan is in
[`../ROADMAP.md`](../ROADMAP.md).
