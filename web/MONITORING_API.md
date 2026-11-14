# Stage D frontend contract (D31–D34)

All requests use `Accept: application/json`, `cache: no-store` and the in-memory
`useAdminSession().token` in an Authorization bearer header. No monitoring data
or credentials are persisted by this workspace. Routes returning errors or HTML
are surfaced as errors; there is no local fallback. Errors show HTTP status,
never potentially sensitive response bodies.

- GET `/api/v1/monitoring/indexers`: array or `{indexers: [...]}` / `{items: [...]}`.
  Each row: `id`, `name`, `enabled`, `status` (enabled, disabled, circuit-open,
  half-open), `requests`, `failures`, `authErrors`, `rssOperations`, `downloads`,
  `meanResponseMs`, `failureStreak`, `backoffSeconds`, `lastEventAt` (ISO date).
  Counters may be nested in `stats`, circuit fields in `circuitBreaker`.
  Common snake_case aliases are accepted. Missing counters display a dash;
  they do not become zero. Totals cover all returned indexers, independent of
  local table filters; the backend should return the complete indexer set.
- GET `/api/v1/monitoring/events?limit=25&offset=0`: `{events: [...], total}`.
  `items` and `data` arrays are accepted, as is an unwrapped array.
  Events expose `id`, `indexerId`, `indexerName`, `kind`, `outcome`, `timestamp`.
  `createdAt` and snake_case aliases are accepted. Raw query, URL, error detail,
  payload and credential fields are intentionally discarded.
  Optional indexed filters: `indexerId`, `kind`, `outcome`, `before`, `after`.
  Date inputs use local time, converted to ISO UTC. Kind options: search, rss,
  download, auth, test, reset. Outcomes: success, failure, auth-error, skipped.
  Unknown event values still render. Agree canonical filter enums with backend.
  `total` is preferred for pagination; otherwise `hasMore` or a full page signals
  next-page availability. Ordering must be newest first with a stable tie-break.
- GET `/api/v1/monitoring/health`: `{dependencies: {database: {status}, ...}}`.
  Also accepts `checks`, `health`, or direct maps/arrays keyed by `id`/`name`.
  Keys: database, indexers, proxies, connectedApps, mail, rssScheduler, systemTime
  (snake_case accepted). Status: healthy, degraded, unconfigured, unknown.
  Unreported dependencies are unknown, not healthy.
- POST `/api/v1/monitoring/indexers/{encoded-id}/reset`: JSON or 204 response.
  Requires server authorization. Frontend confirms the action and only offers
  reset for circuit-open/half-open rows after existing `/api/v1/auth/me`
  confirms `user.role` is owner/admin. Legacy-token and disabled-auth modes
  without an explicit role do not expose reset. Reset does not enable indexers.

Manual Refresh updates all sections. Optional 30-second polling runs while the
page is visible. Effects cancel on token, route/filter changes and unmount;
reset requests also cancel on token change/unmount. Requests have a 20-second timeout. Independent section errors
allow working sections to remain useful. No event-stream endpoint is assumed.
