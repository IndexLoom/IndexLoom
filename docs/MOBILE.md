# Android and iOS architecture: feasibility, not a release commitment

IndexLoom currently consists of a **Rust 2024 workspace** (core, definitions,
engine, storage, Axum server/importers) and a **React/TypeScript/Vite web UI**.
It is **not an all-Rust UI** and is **not currently an APK or IPA project**.
This document records a migration strategy and explicit blockers instead of
claiming mobile builds exist.

## Recommended rollout

### 0. Install the existing PWA (no native binary)

The current responsive web UI can be installed from a trusted, HTTPS-hosted
IndexLoom deployment on supported Android/iOS browsers. It continues talking
to the remote IndexLoom server, which executes searches and stores tracker
credentials. This is the lowest-maintenance way to use IndexLoom on mobile.

Do not put indexer credentials into the PWA bundle, and never embed an
administrator API token in a distributable build.

### 1. Native thin client using Tauri 2

A mobile client may reuse the React UI and a **small Rust Tauri host**, while
an existing Linux/Docker/Windows IndexLoom server remains authoritative.
App packaging will produce APK/AAB on Android and an Xcode app/archive
(exportable as signed IPA) for iOS.

Needed changes BEFORE first build:

1. Introduce a single configured API transport. Presently frontend requests
   use root-relative paths such as `/api/v1/search`; an app-bundled
   `tauri://localhost` or asset WebView origin will not automatically have
   the same origin as the remote server.
2. Choose and security-review transport: (a) trusted **HTTPS remote origin**
   within the WebView, limited to approved hosts, or (b) bundled React
   frontend plus native Rust network commands / a strict local API proxy.
   The latter requires a proxy, handling CORS/preflight/origin and
   authentication deliberately, not relaxing CORS globally.
3. Review authentication and OIDC callback redirects. Store long-lived
   credentials via mobile platform secure storage rather than freely
   accessible WebView storage where practical.
4. Make the mobile navigation and indexer editor responsive, support
   Android back navigation, handle safe areas, and test on physical devices.
5. Use a Tauri 2 Android/iOS project with platform permissions scoped to
   what is needed. Keep upstream IndexLoom source and UI shared.

Only the Tauri host, mobile permissions, and system integrations need
Kotlin/Swift glue where the platform APIs require it. **Do not rewrite the
whole Rust engine or React UI in Kotlin or Swift.**

### 2. Optional *fully local* mobile execution

A much larger undertaking: compile portable Rust crates for
`aarch64-linux-android` and `aarch64-apple-ios` and embed the engine as
a Rust library exposed via Tauri commands (or another FFI layer). The
existing Axum *daemon* cannot be treated as an ordinary mobile background
service, and an app cannot run an independent desktop-style binary forever.

Assess the following per crate:

- **indexloom-core, definitions, engine:** potentially shareable Rust logic,
  but confirm target support of regex/HTML parser, reqwest TLS, DNS, proxy
  and cookie libraries.
- **indexloom-storage:** per-app writable SQLite database, secure migration,
  backup/restore and hardware-backed secret storage. C SQLite linking
  must work for the device targets, not just Linux.
- **indexloom-server:** break HTTP route handlers' business logic into
  reusable services; don't assume a long-running localhost listener is
  appropriate on a suspended iPhone.
- **Cloudflare challenge helpers / FlareSolverr:** keep as **external**
  services; a headless browser sidecar should not be assumed to run on iOS.
- **Scheduling/background checks:** mobile OS lifecycle, battery and app
  background execution constraints mean that periodic scraping is not
  reliable unless a platform-approved mechanism supports it. Background
  tasks must not be promised to run continuously.
- **Networking and user consent:** HTTPS, private tracker traffic, OIDC,
  platform-specific internet permissions / iOS App Transport Security.

A local-only proof of concept should come **after** a thin-client
release, with distinct capabilities documented on each platform.

## Tooling and CI

Official Tauri 2 references:

- https://v2.tauri.app/start/prerequisites/
- https://v2.tauri.app/distribute/
- https://v2.tauri.app/distribute/google-play/
- https://v2.tauri.app/distribute/sign/ios/

Illustrative commands **only after adding a Tauri host**, not runnable
on today's repo as-is:

```sh
rustup target add aarch64-linux-android
pnpm tauri android init
pnpm tauri android build --apk --split-per-abi

# On macOS with Xcode, provisioning and iOS Rust targets:
rustup target add aarch64-apple-ios aarch64-apple-ios-sim
pnpm tauri ios init
pnpm tauri ios build
```

Use GitHub Actions on `ubuntu-latest` for Android (SDK + NDK) and
`macos-latest` for iOS (Xcode). A compile-only job is not equivalent to a
**signed and installable IPA**: installing on a device needs a suitable
signing identity and provisioning profile. Signing secrets must stay out
of the repository and build logs.

Apple's free Personal Team can test on a user's devices but does not grant
App Store distribution or unlimited signing. As documented by Apple, its
provisioning expires after 7 days. Paid Apple Developer membership and
distribution entitlements are separate.

## Decision

Near term: stabilize backend, API and PWA. Mobile thin client is feasible
without a rewrite, but **do not add placeholder mobile build badges,
nonworking APK/IPA workflows or embedded secrets** before an actual
Tauri host has been implemented and validated.
