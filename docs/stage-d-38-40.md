# Stage D: native update management, events and catalog compatibility

## Updates (D38)

All management endpoints inherit the normal bearer/session authorization and role checks:

- `GET /api/v1/system/updates`: current build version, latest recognized version,
  update availability, cache timestamp and verification/install policy.
- `GET /api/v1/system/updates/history`: up to 100 published releases, assets,
  sizes, release notes and links. Both accept `?prerelease=true`; stable releases
  are the default. GitHub draft releases are excluded.

Settings → Updates works in the web UI and native web-enabled packages. Checks
use a five-minute SQLite cache (`0016_stage_d_updates.sql`). Concurrent checks
are serialized; failed refreshes are throttled too. A failed upstream request
retains the last successful metadata and its original timestamp. An old timestamp
therefore means cached information, not a successful recent check.

`INDEXLOOM_UPDATE_REPOSITORY` defaults to `IndexLoom/IndexLoom`; only an
`owner/repository` GitHub identifier is accepted. HTTP callers cannot supply URLs
or repositories. Requests use reqwest, a ten-second connect timeout, a two-minute
request timeout, bounded responses and explicit validated redirect hops. HTTPS
GitHub/API/release-asset hosts on port 443 are the only destinations. No user
credentials are sent to GitHub. JSON is limited to 8 MiB and packages to 512 MiB.

Native Linux, Windows and macOS server binaries support these commands without
loading the server database, catalog, master key or normal runtime flags:

```sh
indexloom-server update check
indexloom-server update check --prerelease
indexloom-server update download v0.4.0 indexloom-linux-x86_64.tar.gz ./new-package.tar.gz
```

Download arguments are an exact release tag, an exact asset name and a **new**
local destination. The package and `SHA256SUMS` (or `SHA256SUMS.txt`) must belong
to that release in the configured repository. A unique checksum entry, exact
asset size and SHA256 match are required before creating the destination.
Existing paths, including the running executable, are never overwritten.
Packages are not extracted, executed, installed or granted executable permissions.
Checksums establish integrity against the published manifest, **not** a publisher
signature; a compromised release account can publish matching malicious files.
The UI labels downloads as not verified locally; use the CLI to verify them.

Stop the native application, preserve its old binary/package and database backup,
and use the OS's installer/package manager to install a verified package. Rollback
is manual using that retained package and an appropriate database backup; migrations
may make a binary-only downgrade unsafe. There is no automatic install/rollback
claim. Android uses platform packages; iOS IPA downloads are refused by the CLI:
use the platform's signed distribution. Unsigned IPA assets may be linked for
signing by developers but are never installed. Docker uses container recreation
or the operator's explicit Watchtower policy; the native updater does not manage it.

Release builds embed `INDEXLOOM_BUILD_VERSION` with the exact release tag.
Development builds fall back to Cargo's workspace version. SemVer precedence
compares numeric nightly date/run components correctly and ignores build metadata.
Unknown tag formats remain in history and are not claimed as newer versions.
A stable `0.3.0` build correctly outranks `0.3.0-dev.DATE.RUN`.

## General events (D39)

`GET /api/v1/events/stream` is an authenticated SSE stream. It accepts the
`Last-Event-ID` header, emits `change` frames with unique process-epoch/sequence
IDs and JSON `{id, kind, timestamp}`, and sends keepalive comments every 15 seconds.
Kinds cover indexers, test outcomes, completed searches, automation, integrations
and settings. Successful API mutations, actual background test completions,
automation work and catalog invalidations emit refresh signals. Payloads exclude
queries, release titles, credentials and configuration values. Events tell clients
to re-read an authorized resource; they are not a replacement for that resource.

The process-local tokio broadcast queue and replay history each hold 256 events.
Subscription and replay snapshots are atomic. An expired/unknown cursor, a fresh
connection or a lagging subscriber receives `event: reset` so clients refresh their
snapshots. Lagged streams close after reset. Process restarts deliberately reset
replay: this is bounded refresh delivery, not a durable audit log. Connections expire
after five minutes so reconnects recheck authorization. Revoked sessions may retain
these nonsensitive refresh signals until expiry; subsequent resource requests still
recheck authorization. For reverse proxies, disable response buffering for this route.

The React hook uses streaming fetch with the bearer Authorization header (never
query parameters), incremental SSE/UTF-8 parsing, last-ID reconnect, capped exponential
backoff with jitter and cancellation on logout/unmount. Indexers, History and the
Updates/Automation/Applications settings panels refresh on relevant signals. Bursts
are coalesced over 300 ms. A textual connection indicator reports reconnect/auth state.

## Real catalog compatibility (D40)

The separate `Real catalog compatibility` workflow runs on relevant PRs, daily
schedule and manual dispatch. It checks out `IndexLoom/Indexers`, Jackett and
Prowlarr/Indexers **separately**. The official Indexers repo does not commit its
generated definitions directory: `tools/prepare_compatibility.py` uses its real
importer to generate canonical YAML and preserve **every** upstream variant,
including overlapping IDs and Prowlarr version subdirectories. No single built-in
sample can satisfy CI's minimum counts (>500 definitions from each upstream).
All three resolved Git SHAs are saved in `revisions.json`; dispatch accepts full
SHAs to replay an earlier artifact. Latest branches are used by default to detect
upstream drift. Third-party tooling runs only in the isolated CI checkout.

To reproduce (Python dependencies are provided by the official catalog's frozen uv environment):

```sh
# Clone each official repository separately, then check out the artifact's SHAs.
uv sync --project /tmp/catalog --frozen --python 3.13
uv run --project /tmp/catalog python tools/prepare_compatibility.py \
  --catalog /tmp/catalog --jackett /tmp/jackett --prowlarr /tmp/prowlarr \
  --output /tmp/compatibility-matrix
cargo run --locked -p indexloom-engine --example catalog_compatibility -- \
  /tmp/compatibility-matrix/canonical /tmp/compatibility-matrix/canonical-report \
  501 0 tests/compatibility/expected.json
cargo run --locked -p indexloom-engine --example catalog_compatibility -- \
  /tmp/compatibility-matrix/variants /tmp/compatibility-matrix/variants-report \
  1002 0 tests/compatibility/variants-expected.json variants
cargo test --locked -p indexloom-engine cardigann::conformance
```

Reports contain every definition's schema/configuration result, source revision,
capability summary, configuration field count and colliding IDs. Original variant
YAML is also checked with the production duplicate-key-rejecting parser. The harness
constructs real Cardigann configurations without sending network requests. Production
HTML/XML/JSON extraction/filter paths run against independently authored recordings.
The workflow also runs the broader behavior conformance suite.

CI fails on too few definitions, any unexpected failure, any stale expected failure,
missing fixture formats or canonical ID collisions. Cross-source/version collisions
are expected in variant mode and are reported rather than discarded. Duplicate
source-project/path identities always fail. Expected failure files map exact relative
paths to exact failure stages (`schema`, `origin`, `runtime-config`, `raw-yaml`);
they start empty. Changes require review; do not increase thresholds to hide regressions.
JSON/Markdown reports, source SHAs and the official importer manifest are uploaded
even on failure. Tracker availability, credentials, arbitrary definition-specific
selectors and all possible engine features are **not** proven by these offline checks.

Network-free checks:

```sh
cargo test -p indexloom-engine catalog_compatibility
cargo test -p indexloom-server events
cargo test -p indexloom-server updates
cd web && node scripts/check-events.mjs
```

The local fixture run intentionally uses a small count; it verifies the harness,
not broad catalog compatibility. The real scheduled/PR job enforces the large counts.
