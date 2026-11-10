# ADR 0004: Build self-contained web and headless Rust binaries

- Status: accepted
- Date: 2026-11-11

## Context

IndexLoom must be installable without Docker on Windows, macOS, and Linux,
and also ship a container image. A server-side API and a browser UI have
different needs: some users want a completely headless service; others want
one executable with the interface included.

## Decision

Build two mutually exclusive Rust feature variants from the same
`indexloom-server` crate:

- `embedded-web`: a build-time generated map embeds compiled web assets into
  the Rust executable with `include_bytes!`. No Node or pnpm is needed at runtime.
- `headless`: only the REST/search API and background catalog sync are served;
  there are no UI/static asset routes.

Default builds retain the external web directory for the existing Docker
image. A GitHub Actions matrix packages both variants for Linux x86-64/ARM64,
Windows x86-64, macOS Intel and Apple Silicon. A separate multi-platform
Docker build publishes tagged and edge images to GHCR.

## Consequences

- The web-enabled binary is a self-hosted browser UI, **not** a native
  desktop window.
- Browser assets are tied to the application version, but indexer
  definitions are loaded independently from the newest catalog release.
- Binary archives include both variants and a copy of the application license.
- No upstream Jackett or Prowlarr runtime is required.
- Release verification includes compilation of every target and a live
  first-start smoke test for the Linux web-enabled executable.
