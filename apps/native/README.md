# IndexLoom native: Android, iOS, Windows, macOS, Linux

This Tauri 2 application includes **two real modes**. Neither mode modifies
an existing remote deployment just by opening it.

- **Autonomous** opens the shared Rust IndexLoom engine on a random local-only
  loopback port with its own SQLite database, encryption master key and
  operator token in the OS-managed app-local directory. The normal React
  IndexLoom UI is served from embedded web assets by the local Rust server.
  A first-run indexer catalog sync runs asynchronously.
- **External** validates a user-entered HTTPS root URL then navigates the
  native WebView to that IndexLoom deployment. In external mode no local
  IndexLoom server, database or tracker requests are started. Login happens
  on the remote site.

The mode chooser is packaged by Tauri. It displays the autonomous operator
token once the local engine starts, so that the user can copy it before
opening the normal login page. To switch modes, restart the application and
select a different mode; the last remote URL is remembered in the chooser.
Remote pages cannot invoke local mode-management commands.

## Build requirements

For Android: Android SDK/NDK, Java 17, Node 24 / pnpm 11, Rust 1.98,
Tauri CLI 2.12.2. For iOS: macOS with Xcode, CocoaPods, Rust iOS targets,
and the same Node / Tauri toolchain.

The GitHub workflow `.github/workflows/mobile.yml` builds the React
assets first (required by `indexloom-server/embedded-web`), then invokes
`tauri android build` on Ubuntu and `tauri ios build` on macOS.

**iOS builds without Apple signing secrets produce an unsigned IPA and
xcarchive, not an immediately installable application.** The IPA must be
signed for the target device by a suitable signing workflow; publishing to
the App Store requires Apple's distribution requirements.

Autonomous runtimes only run while the native app is allowed to execute.
Android/iOS may suspend them in the background. FlareSolverr, if configured,
remains a separate network service: no headless browser is bundled into iOS.

The native Tauri project has its own Cargo workspace and lockfile lifecycle;
it does not add Tauri's GUI dependencies to the core headless/server builds.
