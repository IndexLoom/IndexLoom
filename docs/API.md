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

## Runtime search metrics

Authenticated `GET /api/v1/metrics` exports process-local Prometheus
counter samples (`indexloom_search_requests_total`,
`indexloom_search_completed_total`, `indexloom_search_results_total`).
Counters reset on service restart and contain **no query text, indexer names,
user identities or other high-cardinality labels**. API authentication and
`Cache-Control: no-store` apply. Configure an authenticated scraper to monitor
the endpoint; unauthenticated public exposure is not recommended.

## Torznab, Newznab, Usenet and downloads

The server has dedicated RSS endpoints for compatibility clients:
`GET /api/v1/torznab`, `GET /api/v1/torznab/{indexer}`,
`GET /api/v1/newznab`, and `GET /api`. The integration API key
`INDEXLOOM_TORZNAB_API_KEY` (32+ random characters) is **separate** from
administrator authentication. Available operations include `t=caps`,
`t=search`, `t=tvsearch`, `t=movie`, `t=music`, `t=book`, and
`t=get&id=<stable-guid>`. Search supports extended media identifiers,
category IDs and RSS attributes. All downloads are proxied via encrypted,
7-day opaque `/api/v1/dl/{ticket}` URLs rather than public tracker URLs.

Usenet sources are configured by the administrator with
`GET /api/v1/usenet/indexers` and `PUT` / `DELETE`
`/api/v1/usenet/indexers/{id}`. Only HTTPS public domain endpoints are
accepted. IndexLoom searches Newznab and safely exports NZB files; it is
not an NNTP downloader.

`GET /api/v1/torrentpotato` or `GET /potato` supplies TorrentPotato-style JSON.
The optional administrator-controlled `POST /api/v1/blackhole/{ticket}` exports
validated `.torrent`, `.nzb`, or `.magnet` files to the volume specified by
`INDEXLOOM_BLACKHOLE_DIRECTORY`. No arbitrary client-chosen filesystem path
or overwrite is allowed.

See [Stage B search/API](STAGE_B_SEARCH_API.md) for example queries,
permissions, deployment, limitations, and credentials guidance.

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

Optional `filters` JSON fields include `mode`, `category`, `genre`,
`year`, `season`, `episode`, `imdbId`, `tmdbId`, `tvdbId`, `tvmazeId`,
`traktId`, `doubanId`, `tvrageId`, `artist`, `album`, `author`,
`title`, `publisher`, `label`, `track`, `group`, `minSize`, `maxSize`,
`maxAgeDays`, and `episodeCode`. `resultOptions` configures bounded
global deduplication, sorting and filtering and the response includes
`uniqueResults`, `duplicateCount`, and `filteredCount` without removing
the original grouped `indexers` results. Standard
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
  "detailsUrl": null,
  "downloadUrl": "/api/v1/downloads/0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
  "magnetUrl": null
}
```

Search responses withhold tracker details/GUID/download/magnet URLs. A usable
release has a server-issued `downloadUrl` handle instead. Fetch it with the same
Authorization credential used for the search; browser navigation alone cannot
supply that header. Handles expire after ten minutes and are single-use.

`GET /downloads/{handle}` returns validated `application/x-bittorrent` bytes or
JSON `{ "magnetUrl": "magnet:?…" }`, without forwarding tracker response headers.
Unknown/expired/consumed handles return 404, excess concurrent downloads return
429, and upstream/validation failures return a generic 502. Configuration or
runtime changes invalidate existing handles. No arbitrary target URL is accepted.
Experimental Torznab currently omits grab links pending feed-key authenticated
download integration. See [download and mirror behavior](STAGE_A_DOWNLOAD_MIRRORS.md).

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
