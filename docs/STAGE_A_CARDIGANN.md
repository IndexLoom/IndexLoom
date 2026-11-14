# Stage A — Cardigann and indexer compatibility

Stage A covers the first 13 gaps identified in the Jackett/Prowlarr comparison. IndexLoom remains an independent Rust implementation; it does not run Jackett or Prowlarr as a backend.

| # | Capability | Implementation and verification |
| --- | --- | --- |
| A1 | Cardigann filters | `cardigann/transform.rs`, `filters.rs`, conformance and malformed-input tests; unsupported filters return errors instead of being silently discarded |
| A2 | Multi-stage downloads | `cardigann/download.rs`; GET/POST, before pages, ordered and nested selectors, validated torrent files and magnet hashes |
| A3 | Request pacing | `transport.rs` request gate, bounded delays, shared across search/login/download operations |
| A4 | Legacy encodings | `transport.rs` wire encoders/decoders for query, form, HTML/XML/JSON responses |
| A5 | Interactive tracker authentication | `cardigann/challenge.rs` and `tracker_login.rs`; manually answered image, text and code challenges with bounded tokens and image reads |
| A6 | Row filters | `cardigann/rows.rs`; `andmatch` and other row-level selection behavior |
| A7 | Preprocessing filters | `cardigann/response.rs` and transform pipeline |
| A8 | JSONPath and XML extraction | `cardigann/jsonpath.rs`, `xml.rs`, response/extract modules and conformance tests |
| A9 | Mirror failover | `cardigann/mirrors.rs`; catalog-only URL pool, health cooldown and bounded transient-error retries |
| A10 | Certificates | `cardigann/transport.rs`; explicit custom TLS CA roots and safe incompatibility errors |
| A11 | Pagination | Page/offset-aware Cardigann search paths, explicit bounds and duplicate-result control |
| A12 | Native indexer adapters | `indexloom-native` FileList, UNIT3D and AvistaZ adapters with fixtures |
| A13 | Durable authorization sessions | `sessions.rs`, `tracker_sessions.rs` and SQLite migration 0010; encrypted, scoped, revision-bound cookie sessions |

## Validation

Run from the repository root:

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --offline -- -D warnings
cargo test --workspace --offline --no-fail-fast
# For an environment with loopback socket access:
cargo test -p indexloom-engine --offline --lib -- --ignored
cd web && corepack pnpm install --frozen-lockfile && corepack pnpm typecheck && corepack pnpm build
```

The two ignored wire tests require loopback sockets and are intended to be run explicitly outside the restricted CI sandbox.

## Security and compatibility boundaries

- An indexer's outgoing requests are restricted to declared origins; no arbitrary private-network URLs or cross-origin credential forwarding.
- Failover is permitted only for catalog mirrors and transient network/HTTP failures; authentication or policy failures do not trigger retries elsewhere.
- Download handles are temporary, credential-bound, single-use capabilities. Private passkeys and original tracker URLs are kept out of search responses.
- Solver cookies obey normal domain and public-suffix restrictions. Sessions are never shared between configured indexer instances and are invalidated on credential revision.
- Interactive login does **not** auto-solve reCAPTCHA, hCaptcha, Turnstile or browser-only JavaScript challenges. Those produce explicit errors; administrators may use their own session cookies or the applicable FlareSolverr integration.
- A valid YAML catalog entry is not a guarantee that the corresponding public or private tracker is available or fully compatible without live, authorized tests.
