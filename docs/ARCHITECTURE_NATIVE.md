# Cross-platform engine separation

The platform boundary is explicit and shared:

- `indexloom-core`: domain types, validated deployment modes
  (`deployment.rs`) and capabilities for Android/iOS/Windows/Linux/macOS/
  Docker (`platform/*.rs`).
- `indexloom-definitions`: tracker YAML definitions and capabilities.
- `indexloom-engine`: Rust Cardigann parser/executor. HTML extraction
  and normalized results live in `cardigann/extract.rs`; template filters,
  CSS selectors, dates and primitive conversion in `cardigann/filters.rs`.
- `indexloom-storage`: SQLite and encrypted credential persistence;
  native paths use `Store::connect_path` instead of a URL string.
- `indexloom-server`: HTTP API for both headless and embedded native
  execution. `local.rs` starts a loopback-only foreground runtime;
  `remote.rs` implements external-only 307 forwarding for CLI/server
  binaries that do not need SQLite or local credentials.
- `apps/native`: Tauri 2 native host, secrets handling, platform project
  generation, and a start screen for the two modes. It **reuses** the
  server's embedded web UI and Rust engine; there is no separate Android
  or iOS tracker scraper.

Files exceeding ~1000 lines were split along behavior or test boundaries;
server/storage/executor tests moved to sibling files and Cardigann
extraction and filter functions moved out of the central module.

## Native lifecycle and security

Autonomous uses an ephemeral `127.0.0.1` port only and saves a random
installation-scoped master key and operator token in the app-local sandbox.
Do not put those tokens into the compiled JavaScript or any remote URL.
Deleting `master.key` while keeping the database can destroy access
to encrypted tracker credentials. Native code refuses silent regeneration
when the database exists without its master key.

External mode only accepts an HTTPS origin root without embedded user
credentials, URL parameters or path. A backend/CLI external-only executable
returns a 307 redirect and **does not act as a general-purpose proxy**.
This intentionally differs from server-mode reverse-proxy functionality:
the browser ultimately talks directly to the remote installation.

The mobile WebView is on the remote site's origin after switching to
External. The packaged native mode chooser has separate Tauri
capabilities and checks its origin before returning local credentials.
Account data must not cross from one endpoint to another.

## Known limitations and follow-up

- Mobile platform builds are compiled in GitHub Actions, not on any user's VPS.
- iOS unsigned IPA must be signed for installation.
- The backend is designed for foreground mobile execution; iOS/Android
  may suspend it whenever the app loses execution privileges.
- Full Torznab/FlareSolverr compatibility still requires separate testing.
- The existing RustSec advisory job reports dependency issues that must
  be addressed independently of mobile packaging.
- Independent native workspace dependencies need their own reproducible
  lockfile before promotion to a stable release.
