# Stage C — application integration and automation

Stage C implements application synchronization, search quotas, durable
scheduling, and RSS release monitoring. Direct download-client integrations,
Grab forwarding and notification providers are deferred by project decision.
The source and its SQLite migrations are versioned independently of the
live deployment; validate migrations and save a backup before upgrading.

## Applications (C23) and profiles (C30)

Implemented adapters use the remote application's own indexer schema, then
GET/POST `/api/{version}/indexer` and PUT/DELETE `/api/{version}/indexer/{id}`.
Sonarr and Radarr use v3; Lidarr and Readarr use v1; Whisparr uses v3.
Other API versions are not automatically guessed or retried. Mylar3 (`mylar`)
and LazyLibrarian (`lazylibrarian`) use GET `/api` command APIs with reqwest query
parameters, not *arr schemas. Their separate dialect modules validate Mylar's
`success/data/torznabs/newznabs` and LazyLibrarian's
`Success/Data/Torznabs/Newznabs` responses. Unknown or unsuccessful responses fail.
Live compatibility with installed applications has not been verified.

Named providers use `listProviders` / `listNabProviders`, `addProvider`,
`changeProvider`, and `delProvider`. Their full per-indexer feed URL is sent as
`host`, the feed key as `prov_apikey`, and the application key as `apikey`.
Default categories are Mylar comics 7030 and LazyLibrarian books 7000; explicit
LazyLibrarian audio categories (3000 family) are supported. Profiles reuse the
same IDs/tags/groups selectors and Disabled/AddOnly/FullSync modes.

Migration 0013 now includes `integration_managed_provider_names`, separate from
*arr mappings. Ownership requires its persisted marker `IndexLoom [integration/local]`,
provider kind and (Mylar) numeric ID. LazyLibrarian uses the exact display name,
never its internal slot name. Creates, updates and deletes are re-listed and
verified before local ownership changes. Cleared LazyLibrarian hosts count as
removed slots. Unrecorded markers are refused, including deselected orphans.
A protocol change or a missing mapped provider requires explicit reconciliation;
a renamed LazyLibrarian display name cannot safely be distinguished from deletion.
Remote priority and tags are *arr-only settings; these named APIs have no such
parameters. Their tests use an in-memory command API and local HTTP fixtures.

Both Torznab and Newznab indexer schemas are supported when the remote application
advertises the corresponding implementation and required fields. Missing schema
capabilities fail explicitly. Provider categories default to TV (5000), Movies
(2000), Audio (3000), Books (7000), and XXX (6000), respectively. Explicit category
IDs are intersected with that provider family. Indexers declaring only other
category families are excluded; unknown categories do not imply incompatibility.

The default profile is Disabled. AddOnly creates missing selected entries and
leaves existing entries untouched. FullSync updates configured fields of selected
owned entries, preserving other remote advanced settings, and removes deselected
owned entries. Ownership requires a persisted local/remote
mapping plus the exact integration/indexer marker and supported implementation.
Entries without a mapping are never adopted, modified or deleted. A reused remote
ID causes a conflict. An unmapped matching marker also causes a conflict, including
when a previous POST succeeded before the mapping could be saved. Resolve that
orphan in the remote application's UI before retrying. Remote mutations and local
mapping writes cannot form a distributed transaction. Configuration mutation
serialization is process-local; simultaneous application edits/syncs from multiple
server processes are not supported. Scheduler job claims themselves are stored
and fenced in SQLite.

Profiles combine IDs, groups and tags by intersection; tags within a dimension
match by union, as do groups. Empty dimensions are unrestricted. Saving validates
limits, identifiers and referenced indexers/groups/tags. Disabled or unavailable
indexers are excluded. One default profile is exposed per application. Priority,
remote application tag IDs and category IDs are validated numerically; remote tag
existence and category subtype availability are ultimately validated by the app.

Administrator-only REST endpoints:

- `GET /api/v1/integrations`
- `GET|PUT|DELETE /api/v1/integrations/{id}` (PUT creates or edits)
- `GET|PUT /api/v1/integrations/{id}/profile`
- `POST /api/v1/integrations/{id}/test` (*arr system/status; named provider list)
- `POST /api/v1/integrations/{id}/sync`

PUT connection body: `provider`, `name`, `origin`, `enabled`, optional `secrets`.
Secrets accept only `apiKey` (remote application's key) and `feedApiKey` (must match
IndexLoom's existing `INDEXLOOM_TORZNAB_API_KEY`). Omitting secrets preserves them;
an empty map clears them. The Settings form replaces both keys together. Keys
are write-only, AEAD-encrypted with integration/provider/origin identity binding,
and excluded from API responses and event records. Origin/provider are immutable.
Deleting an integration deletes local ownership metadata; remote entries remain.

Example profile (no credentials):

```json
{
  "mode": "AddOnly",
  "feedOrigin": "http://127.0.0.1:3000",
  "indexerIds": [], "groupIds": [], "tags": [], "categories": [5000],
  "remoteTags": [], "priority": 25
}
```

`feedOrigin` is the IndexLoom origin as reachable by the remote application, not
the application's own origin. Origins have no path, query, fragment or embedded
credentials. Plain HTTP is restricted to explicit loopback IPs or localhost.
Other origins require HTTPS. Before application requests, DNS resolves once,
all addresses are checked, and that exact address set is pinned to a dedicated
client. Private, link-local, multicast, mapped and reserved addresses are rejected
except explicitly configured loopback or private addresses under explicit LAN opt-in.
LAN names must be single-label Docker names or end in `.local`/`.internal`;
public DNS names cannot use this exception. Metadata and link-local destinations
remain forbidden. HTTP private names must resolve entirely to private addresses. Redirects, environment proxies and cookies
are disabled. Response bodies are bounded and upstream failure text is not exposed.
By default, private LAN addresses are rejected. To reach a trusted Sonarr/Radarr
container over Docker RFC1918 networking, set
`INDEXLOOM_INTEGRATION_ALLOW_LAN=true` on the IndexLoom **server**. With this
explicit operator opt-in, HTTP is permitted for private numeric IPs, single-label
Docker DNS names, and .local/.internal names; HTTPS can also resolve to RFC1918
or IPv6 unique-local addresses. DNS results are still pinned, redirects disabled,
and link-local/metadata addresses prohibited. Grant this opt-in only to trusted
administrators; it broadens IndexLoom's outbound access to your LAN.
Subpath installations remain unsupported.

## Quotas (C26)

GET `/api/v1/automation/quotas`; PUT `/api/v1/automation/quotas/{indexerId}` with
`hourlyLimit` and `dailyLimit`. Zero means unlimited; no quota row also means
unlimited. Fixed windows reset on UTC hour/day boundaries. A single conditional
SQLite update reserves both budgets immediately before an uncached upstream
search. Failed attempts consume quota; cache hits do not. Interactive, streaming,
feed and scheduled RSS searches share this path. Exhaustion fails that indexer,
while other indexers can still complete. Storage errors fail closed.
The quota listing includes `retryAfterSeconds` and `nextAllowedAt` while
an indexer is blocked; for combined hourly/daily exhaustion this is the later
of the two UTC reset boundaries.

## Scheduler (C27)

GET `/api/v1/automation/jobs`; PUT or DELETE `/api/v1/automation/jobs/{id}`.
POST `/api/v1/automation/jobs/{id}/run` schedules an enabled, idle task
to execute on the next scheduler poll (without waiting for its interval).
PUT fields: `kind`, `target`, `intervalSeconds` (60..2592000), `enabled`.
Kinds: `app-sync` (integration ID), `rss` (indexer ID), `maintenance` and `catalog`
(the latter two require null/omitted target). Saving schedules the next run one
interval from now. Last run/outcome and next run are visible in the API and Settings.

The executable starts one worker after initialization. The worker polls every ten
seconds and executes jobs sequentially. Claims use a durable 300-second lease and
unique fencing token; execution times out after 240 seconds. Crashes leave a job
eligible after lease expiry; missed intervals coalesce into one run. This provides
at-least-once attempts, not exactly-once remote effects. Jobs cannot be edited or
deleted under a live lease. Long jobs may partially complete before timeout.

Maintenance and catalog defaults are inserted only when absent, preserving saved
edits across restarts. `INDEXLOOM_CATALOG_SYNC_SECONDS=0` prevents initial catalog
job creation; an already persisted job is controlled through the API. Catalog jobs
use the existing configured CatalogSource. Maintenance prunes expired auth state
and caps integration events/RSS history at 10,000 rows each. Existing startup
catalog initialization remains in place. The native foreground runtime starts the scheduler and stops it when the
native instance is dropped; mobile OS suspension may pause all scheduled work.
Standalone embedders building their own router must explicitly start the
`automation::run` worker; router construction alone does not start tasks.

## RSS state/history (C28)

RSS jobs fetch up to 200 recent entries per indexer through its existing search
executor. The first successful scan atomically records a baseline and seen IDs
without creating history entries. Later unseen identities create durable history;
repeats do not. Failed scans preserve the previous state. Stable identity digests
are stored instead of upstream URLs, credentials or temporary result UUIDs.
GET `/api/v1/automation/rss/state` exposes baseline and last successful scan times.
GET `/api/v1/automation/rss/history` returns the latest 500 entries. Seen identities
are retained to avoid rediscovering old entries after history pruning. This set
can grow; no retention policy for it is implemented yet. Scan coverage is bounded
to the upstream's returned recent window and does not guarantee a complete backlog.
No download tickets are created by scheduled scans.

## Settings and deferred scope

Settings includes Applications for connection CRUD, test, profile editing and
manual sync; Automation manages jobs, quotas, outcomes and RSS history. Existing
styles, native labels, fieldsets and status/error regions are reused. Browser,
keyboard and screen-reader checks were not run in this restricted task.

Downloader integrations, Grab sending and notification providers are deferred.
There are no new routes or adapters for them. Existing feed/torrent retrieval and
existing authentication email features predate this change and remain intact.
No dummy adapters, deployment, external lookup or dependency installation is needed.

## Verification

The implementation was tested in an isolated server-side source checkout
without modifying the production SQLite database or Docker deployment.

- `cargo fmt --all -- --check` and `git diff --check`
- `cargo clippy --workspace --all-targets --offline -- -D warnings`
- `cargo test --workspace --all-targets --offline`: **all executed tests
  passed**, including all HTTP/loopback tests (two legacy engine wire tests
  remain ignored by their original test annotations).
- `corepack pnpm install --offline --frozen-lockfile --ignore-scripts`
- `corepack pnpm typecheck`, `corepack pnpm i18n:check`,
  `corepack pnpm build`

All seven integrations have fixture/conformance tests. Live authenticated
testing against actual third-party application installations, browser E2E
automation and network-specific deployment configurations remain outstanding.

The scheduler intentionally provides at-least-once execution, not a
distributed exactly-once guarantee. A reused mapping ID, unexpected external
change or disabled source fails explicitly rather than modifying unrelated
indexers. A catalog or API build does not guarantee that every external
application version or private tracker is compatible.
