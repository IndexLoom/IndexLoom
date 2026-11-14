# Native indexer adapters (stage A, A12)

`indexloom-native` is a standalone Rust workspace crate. It implements native
request formation, authentication, JSON parsing, release extraction, explicit
pagination and torrent downloading. It does not depend on `indexloom-engine`,
Cardigann, or a running Jackett/Prowlarr service. All three adapters are compiled
by default; there are **no feature flags**, no optional socket client, and no
placeholder site registrations.

## Supported protocols and readiness

| Site ID / credentials | Search and authentication | Downloads | Readiness |
| --- | --- | --- | --- |
| `filelist` / `Credentials::FileList` | `api.php`, Basic username:passkey; latest, text or `IMDb` search; native category IDs, season/episode, freeleech | Construct `download.php?id=…&passkey=…` on the configured origin | Implemented, fixture tested; page 1 only (API has no pagination) |
| `unit3d` / `Credentials::Unit3d`, `Unit3dApi::Legacy` | `api/torrents/filter`, `api_token` query; text, `IMDb`, native categories, episode text; boolean freeleech | API's signed `attributes.download_link` | Implemented, fixture tested against Prowlarr's legacy wire shape |
| `unit3d` / `Credentials::Unit3d`, `Unit3dApi::Modern` | Same endpoint, bearer token; structured season/episode, `IMDb`, categories, `free[]=100`; percentage freeleech and featured/internal flags | API's signed download link | Implemented, fixture tested against Jackett's UNIT3D 9.2 definitions |
| `avistaz` / `Credentials::Avistaz` | POST username/password/PID to `api/v1/jackett/auth`; bearer search on `api/v1/jackett/torrents`; text/`IMDb`, movies/TV, season/episode, freeleech | API's signed RSS download link with the instance bearer | Implemented, fixture tested including token expiry and renewal |

`unit3d` is a configurable **framework protocol ID**, not a claim that every
UNIT3D deployment is supported. Select the actual API variant and supply the
site's base URL and category mapping. No automatic aliases for Aither, Blutopia,
`PrivateHD`, `CinemaZ` or other forks are registered. The `AvistaZ` implementation
targets `AvistaZ` itself (movies/TV, native IDs 1/2); it does not claim Exoticaz's
different categories or other forks' title rules.

These implementations are ready for host transport/dispatch integration and
credentialed live validation. They have **not been tested against real private
tracker accounts**. There is no server route, catalog dispatch or engine wiring
in this changeset, intentionally: the parallel engine workstream integrates
after cherry-pick. A12's broader native coverage remains incomplete: `RuTracker`
and Gazelle are studied but **not implemented or advertised**. No HTML fallback,
CAPTCHA handling, Gazelle freeleech-token spending, daily-date episode search,
TMDb/TVDb search inputs, genre search, or automatic host/category discovery is
provided. Unsupported fields are absent from the public query API.

## Entrypoint and shared types

Store one `NativeAdapter` per configured instance behind `Arc<dyn NativeIndexer>`.
Construction validates config/credentials and performs no network I/O.

```rust
use std::sync::Arc;
use indexloom_native::{
    Credentials, Error, InstanceConfig, NativeAdapter, NativeIndexer, NativeQuery,
    RequestPolicy, Secret, ValidatedTransport,
};

fn configure_filelist(
    transport: Arc<dyn ValidatedTransport>,
    username: String,
    passkey: String,
) -> Result<Arc<dyn NativeIndexer>, Box<dyn std::error::Error>> {
    let config = InstanceConfig {
        instance_id: "filelist-primary".into(),
        base_url: "https://filelist.io/".parse()?,
        policy: RequestPolicy::default(),
    };
    Ok(Arc::new(NativeAdapter::new(config, Credentials::FileList {
        username: Secret::new(username),
        passkey: Secret::new(passkey),
    }, transport)?))
}

async fn search_and_download(adapter: &dyn NativeIndexer) -> Result<(), Error> {
    let page = adapter.search(&NativeQuery {
        term: "Example film".into(),
        native_categories: vec![4], // `FileList` Movies HD; host maps catalog IDs
        limit: 25,
        ..NativeQuery::default()
    }).await?;
    if let Some(release) = page.releases.first() {
        let torrent = adapter.download(release).await?;
        // Deliver torrent.as_bytes() through the host's authenticated endpoint.
        assert!(!torrent.as_bytes().is_empty());
    }
    Ok(())
}
```

`NativeQuery::from_core(&SearchQuery, mapped_native_categories)` copies text and
limit but requires an **explicit mapping** from normalized core categories to
tracker-native IDs. Core `SearchQuery.categories` are never guessed to be native
IDs. `NativeRelease.result` uses the existing `indexloom_core::SearchResult`:
instance/site identity, native ID, details GUID, title, category label, byte size,
seeders, leechers, normalized info hash when supplied, and publication time.
`ReleaseMetadata` adds file/grab counts, external IDs, volume factors, internal
status, and `AvistaZ` audio/subtitle languages. Category values in results remain
native labels; normalization belongs to the catalog integration.

`NativeRelease` is also an opaque, immutable download handle bound to its exact
adapter object. A handle from another object is rejected, even if its configured
instance ID is the same. Keep the adapter object alive across search/download;
rebuilding it intentionally invalidates old handles. `FileList` and legacy UNIT3D
auth are stateless. `AvistaZ`'s bearer is memory-only, serialized with instance
requests, reused across searches/downloads, and renewed at most once after
401/412. Failed renewal clears the token so the next operation starts cleanly.
There is no shared global session/cache or token persistence.

**Signed URLs are credentials.** `result.download_url` is deliberately `None`;
the host should expose a native download route using retained handles. The
explicit `download_url()` accessor is sensitive and is provided for integrations
that need the original link. Adapter/request/response/release/torrent Debug
implementations exclude raw bodies, headers and URLs; secrets use zeroizing
storage. No tracing or logging occurs inside this crate. Do not log raw DTO
details URLs, JSON fixtures replaced with real responses, exposed secrets or
transport-library errors. The transport sees secrets to carry out its task.

## Egress security contract (required integration)

The only network entrypoint is `ValidatedTransport::execute(HttpRequest)`. There
is **no default HTTP implementation**, and this crate never creates a reqwest
client or opens sockets. The host must implement the documented trust boundary:

- Apply its validated egress/SSRF policy on **every attempt**, including login,
  retry, token refresh and download. Resolve and pin approved IPs to connections;
  reject private/reserved/link-local/metadata destinations by default. Account
  for DNS rebinding, proxies, IPv4-mapped IPv6, and proxy-side resolution. A URL
  scheme/hostname check alone does not meet this contract.
- Disable redirect following. Return the original 3xx; adapters reject it.
- Enforce request timeout and **decompressed** streaming response byte limits
  before buffering. Dropping the future must cancel I/O. Do not add hidden
  retries. The adapter's post-buffer length check is a second line of defense,
  not a substitute for the streaming limit.
- Disable ambient cookies/auth. If cookies are needed in a host transport,
  isolate them by the unique `HttpRequest.session_id`, not hostname or user label.
  Do not log URLs, authorization headers, forms, response bodies, or raw errors.

Adapters require clean HTTPS base URLs (reverse-proxy directory prefixes work)
and accept API-provided links only on the configured origin, including scheme
and port, without userinfo/fragments. Cross-origin CDN/download mirrors are
rejected, not followed with credentials. The caller still validates DNS and
destination addresses; same-origin checks alone are **not SSRF protection**.
`FileList` rebuilds downloads on its current configured origin because old API
responses can carry retired hostnames. UNIT3D/`AvistaZ` continuation URLs are
validated for origin, endpoint and page, then discarded: the next call rebuilds
the request from caller query/config, preserving the actual credentials.

The engine's validator currently lives in the engine crate; integrating it into
a transport is the host's follow-up. A naive network implementation of this
trait is not a security validator. The crate cannot enforce the internals of an
arbitrary caller-provided trait implementation.

## Bounds, errors and protocol details

- One API page per search. Query limits 0..=500 (`AvistaZ` 0..=50); zero performs no
  I/O. Pages 1..=10,000; `FileList` rejects page >1. Query text <=1,024 bytes, at
  most 50 native categories; malformed `IMDb`/episode inputs fail before I/O.
  Optional seasons/episodes are numeric, including `AvistaZ`'s episode-only `E05`.
- Defaults: 30 s per attempt, at most three GET attempts, 500 ms retry delay,
  30 s maximum accepted `Retry-After`, 2 MiB JSON and 16 MiB torrent bodies.
  Hard maxima: 120 s/attempt, three attempts, 8 MiB JSON, 32 MiB torrent bodies.
  Operations are also bounded by one `AvistaZ` reauthentication cycle. Each new
  search call is a new operation; there is no cross-call circuit breaker.
- GET retries: transient network failures, timeouts, HTTP 429/502/503/504 only.
  POST login is never retried automatically. Retry-After delta seconds is
  honored up to the configured bound; larger or nonnumeric/HTTP-date values
  immediately return `RateLimited` for host scheduling. All attempts obey
  instance pacing: default 1 s, at least 6.1 s for `AvistaZ` even if configured lower.
  The host must coordinate limits across multiple instances of the same site.
- Returned pages sort by descending publication time and truncate to the
  requested limit; `truncated` records omitted rows. A next page is explicit,
  with no automatic traversal. If a server ignores perPage, continuing with a
  smaller client limit may skip omitted rows: requery that page with a larger
  limit or retain all rows. Server-side category/`IMDb`/text filtering is trusted;
  freeleech is additionally checked locally. `AvistaZ` native categories are also
  checked locally when type=0 is needed for multiple categories.
- `FileList`'s timezone-less timestamps use fixed +02:00, matching both audited
  implementations, including summer dates. UNIT3D's RFC3339 dates honor their
  offsets; legacy bare timestamps are interpreted as UTC. `AvistaZ` uses its
  explicit `created_at_iso`. All outputs normalize to UTC.
- UNIT3D modern percentage freeleech supports 0/25/50/75/100%, including numeric
  percentages, with featured implying free download/double upload. Legacy
  supports booleans/0/1. `FileList` boolean/integer flags support freeleech, double
  upload and internal. `AvistaZ` retains the API's numeric volume multipliers.
- Downloaded bytes must be a complete, bounded bencoded dictionary containing
  an info dictionary; HTML login pages, invalid/truncated bencode, trailing data,
  excessive recursion and excessive token counts are rejected. This is a
  structural metainfo check, not piece verification or a guarantee that a remote
  tracker linked the expected torrent. Downloads do not spend freeleech tokens.

`Error` variants are stable redacted categories: `Configuration`, `Query`,
`Authentication`, `Api`, `RateLimited`, `Http(status)`, `Redirect`, `UnsafeUrl`,
`ForeignRelease`, `Parse`, `Timeout`, `BodyTooLarge`, `Transport`. Context strings
are static, never upstream bodies/URLs. `TransportError` is a closed enum:
`PolicyDenied`, `Transient`, `Timeout`, `BodyTooLarge`, `Permanent`; callers must
map library errors without preserving sensitive diagnostic strings. Malformed
required fields fail the search instead of presenting a partial success.
`AvistaZ` 404 search results are empty; other adapters treat 404 as an error.

## Verification and source provenance

```sh
cargo fmt -p indexloom-native --check
CARGO_BUILD_JOBS=2 cargo test -p indexloom-native --offline
CARGO_BUILD_JOBS=2 cargo clippy -p indexloom-native --all-targets --offline -- -D warnings
```

Tests use a deterministic, in-memory async `ValidatedTransport` and Tokio's paused
clock. No private tracker, live credentials, localhost server, or internet is
needed. Fixtures are independently authored, redacted protocol examples, not
copied upstream test payloads. See [SOURCES.md](SOURCES.md) for exact audited
revisions, behavioral decisions and remaining coverage gaps.
