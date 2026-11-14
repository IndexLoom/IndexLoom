# Stage B — Search, RSS compatibility, Usenet and outgoing downloads

Stage B extends the self-hosted IndexLoom server without relying on Jackett or Prowlarr. The legacy grouped search output, existing 10-minute credential-bound download capability, and Cardigann torrent protocol remain available.

## Features and endpoints

| Area | API and implementation |
| --- | --- |
| 14 Torznab / Newznab | `GET /api/v1/torznab`, `GET /api/v1/torznab/{indexer}`, `GET /api/v1/newznab` and `GET /api`; `t=caps`, `t=search`, `t=tvsearch`, `t=movie`, `t=music`, `t=book`, `t=get&id=<stable-guid>`. Individual APIs select a single configured indexer. RSS outputs category, size, seeders, peers, hash, file count, grabs and optional media fields. |
| 15 Usenet / NZB | Native Newznab source in the indexer manager: `GET /api/v1/usenet/indexers`, `PUT /api/v1/usenet/indexers/{id}`, `DELETE /api/v1/usenet/indexers/{id}`. HTTPS domain and encrypted API-key requirements; bounded Newznab RSS extraction and validated NZB downloads. |
| 16 Search criteria | TVDb, TVMaze, Trakt, Douban, TVRage, artist, album, author, publisher, label, track, title, group, episode-code, minimum/maximum size and maximum age. Structured ID-only searches do not inject wildcard text. |
| 17 Result model | `protocol` plus typed `attributes`: file count, grabs, poster, genres, languages, subtitles, creators/publisher, ratios, seed time and source IDs. |
| 18 Dedupe | Configurable global distinct-result list with title-and-size fallback, torrent info hash or scoped source GUID and indexer priority; original per-indexer lists are retained. |
| 19 Sort / saved filters | `resultOptions` in search requests and `GET /api/v1/search/presets`, `PUT /api/v1/search/presets/{id}`, `DELETE /api/v1/search/presets/{id}`. Presets are isolated to the authenticated user and limited to 32 per user. |
| 20 Cache | In-process, 120-second TTL, at most 256 entries of 256 KiB each. Keyed by source, full structured query, limit and hashed authenticated credential. An executor change fences old entries. |
| 21 Server downloads | Authenticated short-lived download handles remain unchanged. RSS uses opaque 256-bit URLs at `GET /api/v1/dl/{ticket}` with encrypted 7-day SQLite records. `t=get&id=<stable-guid>` resolves the newest valid ticket. Revoked on indexer settings change or disable. |
| 22 TorrentPotato / Blackhole | `GET /api/v1/torrentpotato` or `GET /potato` for TorrentPotato-style JSON and `POST /api/v1/blackhole/{ticket}` to export torrent/NZB/magnet files to an administrator-configured watch directory. |

## API keys and network safety

Torznab/Newznab consumers require `INDEXLOOM_TORZNAB_API_KEY` (32–512 random characters). TorrentPotato can use a separate `INDEXLOOM_TORRENTPOTATO_API_KEY` or fall back to that key. These public-by-design integration routes do **not** accept the ordinary administrator token instead. A GET URL carries the integration key in the query string; use HTTPS and redact query strings from reverse-proxy, access and error logs.

Set `INDEXLOOM_PUBLIC_URL=https://your-host.example` to construct absolute enclosure/download links. Reverse proxies must pass the public host and route `/api/v1/dl/` to the service. The `dl` token is a bearer capability: possession is sufficient for a download for seven days. Do not publish or log it. SQLite stores encrypted release URLs; keep the server master key stable across upgrades.

Usenet source endpoints require a public HTTPS DNS name (not raw IP, local domains, or URL credentials); origin requests also pass the existing transport checks for DNS and private network blocks. Source secrets are write-only and encrypted.

To enable Blackhole exports, mount a **dedicated container path** to a trusted watch folder and set `INDEXLOOM_BLACKHOLE_DIRECTORY=/path/inside/container`. The endpoint is an authenticated administrator action; exports use random token-based filenames and atomic link creation. It never accepts a filesystem path in the HTTP request. Do not mount the Docker socket or writable host system directories for this feature. It is disabled if the variable is unset.

### Example structured search

```json
{
  "query": "",
  "selection": {"all": true, "groupIds": [], "indexerIds": []},
  "limitPerIndexer": 50,
  "filters": {"mode": "movie-search", "imdbId": "tt1234567"},
  "resultOptions": {
    "deduplicate": true, "sortBy": "seeders", "descending": true,
    "minSeeders": 3, "categories": [], "includeIndexers": [],
    "excludeIndexers": [], "indexerPriorities": {}
  }
}
```

Results include an additional `uniqueResults` array, `duplicateCount` and `filteredCount`. The original `indexers` grouped results are unchanged.

### Example Newznab API

```text
GET /api/v1/torznab?t=movie&imdbid=tt1234567&cat=2000&apikey=<key>
GET /api/v1/torznab/usenet-example?t=tvsearch&tvdbid=42&season=1&apikey=<key>
GET /api/v1/torznab?t=get&id=<64-hex-character-stable-guid>&apikey=<key>
```

Compatibility depends on the supported modes of each real indexer. Unsupported per-indexer search criteria may still produce an error rather than a complete result; the built-in Newznab interpreter does not perform browser automation. IndexLoom is not an NNTP client and does not download Usenet articles: it searches NZB indexers and exports the NZB for a separate client.

## Validation

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --offline -- -D warnings
cargo test --workspace --offline --no-fail-fast
cd web && corepack pnpm typecheck && corepack pnpm i18n:check && corepack pnpm build
```

Two legacy HTTP wire tests are ignored in sandboxed test environments. Server API integration tests cover credential redaction and authorization. SQLite migration tests verify encrypted ticket creation and revocation. Neither the finite fixture-based tests nor parsing 589 catalog YAMLs establishes live compatibility with every private tracker or every third-party media manager.
