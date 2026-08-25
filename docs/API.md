# Management and search API

IndexLoom's pre-alpha API is rooted at `/api/v1`. Except for `GET /health`, all
routes require `Authorization: Bearer <INDEXLOOM_ADMIN_TOKEN>` and return
`Cache-Control: no-store`.

## Catalog instances and organization

- `GET /indexers` lists redacted indexer state, configuration schema, tags,
  groups, routing and the latest persisted test.
- `PUT /indexers/{id}/configuration` saves public definition values,
  `User-Agent`, FlareSolverr selection and three-state secrets. A missing secret
  is preserved, a string replaces it and `null` clears it.
- `PUT /indexers/{id}/organization` saves display name, enabled state, explicit
  proxy, group IDs and tags.

New catalog instances are disabled by default. Tests and search only select
enabled instances.

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
- `POST /search` accepts `query`, `selection`, and `limitPerIndexer` (1-200).
  Its `indexers` array always groups results and errors by indexer so one failed
  tracker does not hide successful results from the others.

Request and response objects are not stable before the first tagged release.
