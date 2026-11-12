# Management and search API

IndexLoom's pre-alpha API is rooted at `/api/v1`. Most management/search
routes require authentication, with modes `token`, `local`, `oidc`,
`hybrid` and an explicitly unsafe `disabled` mode. The public exceptions
are `GET /health` and the login/recovery bootstrap endpoints below.
All authentication responses use `Cache-Control: no-store, private`.
See [Authentication and email](AUTHENTICATION.md) for setup and security.

## Catalog instances and organization

- `GET /indexers` lists redacted indexer state, configuration schema, tags,
  groups, routing and the latest persisted test. The `capabilities` object
  summarizes original Jackett/Prowlarr metadata (language, source, category
  IDs and mappings, search modes/parameters, CAPTCHA and login declarations,
  search-path and alternate-URL counts). Declared capability is not a guarantee
  of full interpreter support.
- `PUT /indexers/{id}/configuration` saves public definition values,
  `User-Agent`, FlareSolverr selection and three-state secrets. A missing secret
  is preserved, a string replaces it and `null` clears it.
- `PUT /indexers/{id}/organization` saves display name, enabled state, explicit
  proxy, group IDs and tags.

New catalog instances are disabled by default. Tests and search only select
enabled instances.

## Experimental Torznab / RSS

`GET /api/v1/torznab?t=caps&apikey=<separate-key>` publishes a minimal
Torznab-compatible capabilities document. `t=search&q=<UTF-8-query>`
returns RSS 2.0 with Torznab attributes, escaped XML, search limits
and optional one-category filtering. Other Torznab search modes are
not yet supported, and the exact compatibility depends on enabled
Cardigann definitions. This integration is experimental.

The endpoint is **disabled by default**. Enable with a separate,
random 32+-byte `INDEXLOOM_TORZNAB_API_KEY` in the server environment;
the normal administrator bearer token is not accepted. Many
Torznab clients include API keys in URL query strings: use HTTPS,
mask the query in proxy logs, and do not expose the port publicly.
The endpoint uses the existing bounded search engine and excludes
disabled/retired indexers. Search errors are currently normalized to
an XML error rather than reporting detailed per-indexer failures.

## Multilingual search

Both `POST /search` and `POST /search/stream` accept the optional
top-level `languages` array (for example `["ru", "ja"]`). This narrows
**configured indexers** by their definition's BCP 47 language tag after
normal group/instance selection. A language matches its exact tag or
regional variants (`ru` matches `ru-RU`). No title or query translation is
performed. The `query` field accepts UTF-8 text in any script.

```json
{
  "query": "千と千尋",
  "selection": {"all": true, "groupIds": [], "indexerIds": []},
  "languages": ["ja"],
  "limitPerIndexer": 50,
  "filters": {}
}
```

Invalid language tags or no matching enabled indexers produce HTTP 400.
This restriction is separate from native Cardigann category/genre parameters.

## Routing and groups

- `GET /proxies`, `PUT /proxies/{id}`, `DELETE /proxies/{id}` manage named
  `http`, `socks4` and `socks5` routes. Passwords are write-only.
- `GET /groups`, `PUT /groups/{id}`, `DELETE /groups/{id}` manage many-to-many
  groups, priority and an optional inherited proxy.
- `GET /routing`, `PUT /routing` read and write `globalProxyId`.
- `GET /flaresolverr`, `PUT /flaresolverr/{id}`,
  `DELETE /flaresolverr/{id}` manage browser-challenge connectors.

Proxy precedence is explicit indexer, highest-priority enabled group, then
global. A missing route means direct HTTP.

## Selection object

Bulk tests and search share this shape:

```json
{
  "all": false,
  "indexerIds": ["example-main"],
  "groupIds": ["anime"]
}
```

Explicit IDs and group members form a deduplicated union. `all: true` selects
all enabled indexers and ignores the two ID arrays.

## Tests and search

- `POST /indexer-tests` accepts a selection and returns `202 Accepted` with a
  job record.
- `GET /indexer-tests/{jobId}` returns progress and per-indexer outcomes.
- `POST /search` accepts `query`, `selection`, `limitPerIndexer` (1-200)
  and an optional `filters` object. Mode and category unsupported by any
  selected indexer produce per-indexer errors rather than silently returning
  unrelated results.
  Its `indexers` array always groups results and errors by indexer so one failed
  tracker does not hide successful results from the others.
- `POST /search/stream` accepts the same JSON and returns
  `application/x-ndjson`. It emits `started`, one `indexer` event per completed
  worker, and a terminal `finished` event. Use streaming `fetch`, not native
  `EventSource`, so the administrator token remains in the Authorization
  header rather than a URL.

Optional `filters` JSON fields are `mode`, `category`, `genre`,
`year`, `season`, `episode`, `imdbId` and `tmdbId`. Standard
Cardigann variables including `.Query.Genre`, `.Query.Year`,
`.Query.Season`, `.Query.Ep`, `.Query.IMDBID` and `.Categories`
receive those values. `category` is a normalized category name offered by
a selected definition and gets mapped to native tracker IDs. Unsupported
per-site modes and unmappable categories are explicit failures for that
site, while other enabled sites may succeed.

```json
{
  "query": "example",
  "selection": { "all": false, "indexerIds": ["example-main"], "groupIds": [] },
  "limitPerIndexer": 50,
  "filters": { "mode": "movie-search", "category": "Movies/HD", "year": 2026 }
}
```

Example stream:

```jsonl
{"type":"started","query":"example","total":2}
{"type":"indexer","indexer":{"indexerInstanceId":"example-main","definitionId":"example","indexerName":"Example","responseTimeMs":280,"results":[],"error":null}}
{"type":"finished","query":"example","completed":2,"total":2,"resultCount":0}
```

Completed indexer-test jobs remain available for at most 24 hours and the most
recent 256 completed records are retained in memory. At most eight bulk test
jobs may run concurrently; their individual requests share one global
16-request budget.

Search response identity is explicit. `indexerInstanceId` is the configured
SQLite instance, while `definitionId` is its catalog definition. The UUID in
`id` is unique only within the returned search data; consumers that persist or
deduplicate results should prefer tracker-native `sourceId`, `sourceGuid` and
`infoHash` when present.

```json
{
  "id": "019d0000-0000-7000-8000-000000000000",
  "indexerInstanceId": "rutracker-through-proxy",
  "definitionId": "rutracker",
  "sourceId": "1234567",
  "sourceGuid": null,
  "infoHash": "0123456789abcdef0123456789abcdef01234567",
  "title": "Example release",
  "detailsUrl": "https://tracker.example/torrent/1234567",
  "downloadUrl": "https://tracker.example/download/1234567.torrent",
  "magnetUrl": null
}
```

`detailsUrl`, `downloadUrl` and `magnetUrl` are intentionally independent.
Callers must not assume the details page is a downloadable torrent resource.

Request and response objects are not stable before the first tagged release.

## Authentication API

Public entry points: `GET /auth/config`, `POST /auth/bootstrap`,
`POST /auth/login`, `POST /auth/oidc/start`,
`POST /auth/oidc/complete`, `POST /auth/email/request-reset`,
`POST /auth/email/reset` and `POST /auth/email/verify`.

Protected management endpoints: `GET /auth/me`, `POST /auth/logout`,
`POST /auth/password`, `POST /auth/email/request-verification`,
`GET /auth/users`, `POST /auth/users`,
`PUT /auth/users/{id}/disabled`,
`POST /auth/users/{id}/verify-email`, `GET /auth/mail`,
`PUT /auth/mail/templates/{kind}`,
`PUT /auth/mail/events/{kind}` and `POST /auth/mail/test`.

```json
{"login":"admin@example.org","password":"long-secret-passphrase","rememberMe":false}
```

`POST /auth/login` returns a random bearer `accessToken`, its
Unix-second `expiresAt`, and public account metadata. Use
`Authorization: Bearer <accessToken>` for protected calls.
Sessions can be revoked; a password change revokes all sessions.
`POST /auth/mail/test` accepts `{"to":"someone@example.org"}`.
Mail template updates accept `subject`, `textBody` and optional `htmlBody`.

In `local` mode, the legacy operator token is only accepted at
`/auth/bootstrap` while the first local account is created. In `hybrid`
the operator token also remains usable for management. In `disabled` all
management APIs become public; never use it on a public server.
