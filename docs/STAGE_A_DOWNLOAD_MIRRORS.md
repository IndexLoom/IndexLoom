# Cardigann downloads and mirror failover (A2/A9)

This implementation is independent Rust code. Behavioral references inspected:
Prowlarr `CardigannRequestGenerator.DownloadRequest` (approximately lines
840–985), Jackett `CardigannIndexer.Download`, and the `magnetdownload`,
`kinozal-magnet`, `limetorrents`, and `torrentdownloads` YAML definitions in the
2026-11-14 local source audit. No C# implementation was copied. The committed
YAML fixtures are independently authored, offline interoperability examples of
those real download-block shapes, with mock HTML and binary payloads.

## Engine API

`CardigannExecutor::resolve_release(&SearchResult)` returns
`DownloadArtifact::Torrent(Vec<u8>)` or `DownloadArtifact::Magnet(String)`.
The row must originate from the caller's trusted search cache, not client JSON.
The executor checks its configured instance ID, definition ID and declared
origins before making a request. `can_resolve_release` checks whether a row has
a usable, policy-allowed target; remote availability is necessarily checked
when the download runs.

Supported download blocks:

- Direct GET/POST, templated inputs and headers (download headers override
  search headers).
- `before` GET/POST, templated path/inputs/headers and `pathselector`.
- `.DownloadUri.AbsoluteUri`, `.AbsolutePath`, `.Scheme`, `.Host` and
  `.Query.<name>`, plus the existing Config/Result template contexts.
- Ordered `selectors`, attributes, filters and `usebeforeresponse`; missing
  matches, invalid torrent pages, 404/410 and temporary candidate failures
  advance to the next selector. Auth and security errors always stop.
- `infohash.hash` / `infohash.title`, including `usebeforeresponse`.
- Nested download pages using the ordered selectors again; a selector can also
  declare a nested `download` mapping. Relative links resolve against the page
  that supplied them.
- HTTP redirects to validated magnets; same-origin redirects retain headers,
  307/308 retain the method and form, and 303 switches to GET.

Cookies are scoped to one configured instance and one mirror. Configured browser
cookies seed the cookie jar once so later Set-Cookie responses can update the
session. FlareSolverr cookies and its browser User-Agent are imported into that
same jar for subsequent direct grabs. No browser session/cookie is serialized in
search or download responses.

The engine validates full bencoding, canonical dictionary keys and integers,
complete input consumption, an info dictionary, file/path and piece metadata,
and v1/v2 metadata shapes. Magnet exact topics must contain a valid hex/base32
BTIH or SHA-256 BTMH. MIME type and a leading `d` are insufficient.

Bounds: 16 MiB per decompressed response, five redirects per request, 10 second
connect timeout, 30 second HTTP timeout, 45 seconds for a redirect chain,
90 seconds for an operation including failover, 24 page requests per download
attempt, and six levels of nested resolution. Bencoding is limited to 32 levels
and 200,000 nodes (including dictionary keys). Private/literal/resolved IP policy
is inherited from the engine's connector-time public DNS resolver. Every
request and redirect also checks the selected origin. Explicit remote-DNS
proxies and FlareSolverr remain administrator-controlled egress boundaries, as
in the existing engine; their remote DNS cannot be pinned by the local resolver.

## Server and Web UI

Search endpoints replace tracker download/magnet links with
`/api/v1/downloads/<256-bit-random-handle>` and retain the original row only in
memory. Tracker details and source GUIDs are withheld as well because
they may contain passkeys. Handles expire after ten minutes, are single-use,
and are bound to the exact Authorization credential that performed the search.
The registry retains at most 4,096 handles, each with at most 32 KiB of release
data; oldest handles are evicted at capacity.

`GET /api/v1/downloads/{handle}` is inside the existing authenticated management
router and follows its role policy. Send the normal Authorization header. The
route accepts no caller-supplied indexer, URL, cookie or request headers. Before
downloading it resolves the current configured indexer again, checks that it is
enabled/available, and compares the cached executor identity. Updating settings,
credentials, routing, catalog, or clearing the runtime cache invalidates old
handles. Failed attempts also consume a handle; search again to retry. Four
downloads can run at once; excess requests receive 429 without consuming a handle.

A successful response is either:

- `application/x-bittorrent`, with a fixed safe attachment filename; or
- `application/json`, with `{ "magnetUrl": "magnet:?…" }`.

Responses are no-store and carry no tracker Set-Cookie/Location headers. Remote
error strings/URLs are not returned by the download endpoint. Unknown, expired,
foreign-credential or already-used handles return 404. An upstream execution or
validation failure returns a generic 502.

The UI displays Download only for server handles and fetches them with its
Authorization header. Torrent bytes are saved using a temporary blob URL;
validated magnet material is handed to the browser. The Authorization credential
is never put in a URL, and the UI never navigates directly to a private grab URL.

## Mirror policy

Search, connectivity tests and downloads use the same mirror pool. Candidates
come only from `baseUrls`, followed by `legacyUrls`, with duplicates removed.
At most three candidates run per operation. The first base is initially
preferred; success is remembered and preferred on following calls. Failed
candidates have a 60-second health penalty. No persistent state/schema changes
are required.

`set_preferred_mirror(&Url)` selects an exact catalog URL for this executor's
lifetime. It cannot add an origin. There is no new persistence setting or URL
picker in this change; a caller can use this API when adding that UI later.

Failover retries connection/body transport failures, timeouts and explicit HTTP
408/500/502/503/504/521–524. It does not retry 401/403, 429, malformed templates,
invalid metadata or URL/DNS-policy failures. A raw manually pasted cookie is
never replayed to another origin. Definitions using login credentials establish
an independent authenticated session on each declared mirror; cookie jars and
authentication freshness are not shared between mirrors. Release paths and
queries are rebased only between catalog mirrors.

## Integration boundaries

- Merge the small call-site changes in `cardigann.rs`, engine `lib.rs`, server
  `execution.rs`/`lib.rs` and the two Web UI files alongside the new modules.
  The other Stage A filter/parser and session work must preserve these calls.
- Redirects and selector links to undeclared CDN/torrent-cache origins are
  rejected. Even another declared mirror cannot receive credentials through a
  redirect; failover must start a separate operation/session. Supporting external
  CDNs requires an explicit credential-free origin policy, not a wildcard.
- Download page extraction uses the existing HTML selector/filter renderer and
  its current encoding/filter support. The concurrent filter work may expand
  those semantics. No interactive CAPTCHA/browser download automation is added;
  binary requests use the direct/proxy session after importing solver cookies.
- Experimental Torznab searches do not mint interactive credential-bound
  handles, and therefore currently omit grab/magnet URLs. A separate feed-key
  authenticated grab protocol is needed before publishing working feed links.
- Handles are process-local and expire on restart or runtime replacement. They
  deliberately do not become durable release identifiers.

All tests use mock transports or local SQLite. The DNS-policy test uses only the
OS localhost mapping. No live private trackers are contacted.
