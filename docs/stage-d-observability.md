# Stage D observability

All monitoring routes and Prometheus exposition use the existing management API
session/token middleware and role checks. Read endpoints accept viewers; circuit
reset requires an administrator. No dashboard request contacts an upstream service.

- `GET /api/v1/monitoring/indexers` returns every configured indexer, including
  instances without observations (zero counters and unknown freshness).
- `GET /api/v1/monitoring/indexers/{id}` returns one instance or 404.
- `POST /api/v1/monitoring/indexers/{id}/reset` resets the automatic circuit and
  audits the action. It never changes the user's enabled preference.
- `GET /api/v1/monitoring/events` accepts `before`, `since` (Unix seconds),
  `indexerId`, `category`, `status`, and `limit` (clamped to 1–200). Pass
  `nextBefore` as the next request's `before`. Events are ordered by descending ID.
  Categories include search, test, download, auth, rss, reset, results, and errors.
- `GET /api/v1/monitoring/health` returns database availability, circuit and
  observation freshness, saved integration status, proxy configuration, scheduler
  progress, and notification/mail configuration. Proxy connectivity, mail delivery,
  and independent clock skew are explicitly unknown where no reference exists.
- `GET /api/v1/metrics` preserves the original process counters and adds
  `indexloom_durable_*_total` aggregates, a response duration summary,
  `indexloom_global_authentication_errors_total`, and `indexloom_open_circuits`.

Migration 0014 owns permanent per-instance counters and a separate audit log.
Response count/sum/average include completed search requests (including cache
hits), tests, downloads, RSS and tracker login operations, both successes and
failures. Result counts are observed rows before final federated deduplication;
RSS operations include scheduled scans and API feed dispatch. Pending manual
login challenges are audited without clearing the failure streak. Rejected circuit
admissions do not inflate request, failure, or response counters. Quota rejections
and locally invalid requests are recorded but do not penalize upstream health. Counters survive restarts and
audit retention. Removing an indexer removes its counters and anonymizes its events.

The audit log retains at most 10,000 rows and 30 days. A SQLite insert trigger
prunes on every insertion, including manual resets; reads also exclude expired
rows during idle periods. Messages and categories come from fixed enums. No
search text, URLs, usernames, credentials, tokens or upstream error strings are
accepted by the observation API. `results` and `errors` events supplement the
operation event; do not sum both kinds when counting operations.

Three consecutive failures open the circuit. Initial backoff is 30 seconds
(120 seconds for HTTP 429), doubles on subsequent failures, and caps at one hour.
HTTP 401/403 and rejected tracker authentication are classified separately from
rate limiting, network/timeout, upstream, and configuration errors. A single
network failure leaves dispatch enabled. After backoff, one atomic SQLite lease
admits a recovery operation; all others wait. Recovery searches bypass the cache.
A success clears failures; a failed recovery increases backoff. Abandoned probes
expire after five minutes. Monitoring writes are transactional; execution logs a
fixed error if an observation cannot be persisted, and new dispatch fails closed
when circuit state cannot be loaded.
