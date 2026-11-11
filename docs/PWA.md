# Mobile, tablet, desktop and PWA

IndexLoom includes a responsive browser UI and installable Progressive Web App.
The built-in web-enabled Rust binary and Docker deployment serve the same
assets, manifest, icons and service worker; the headless binary serves only
the API.

- Phone layouts (320–640 CSS px): bottom touch navigation, viewport-safe
  areas for iPhone notches/home bars, 44px-class controls, stacked configuration
  panels and horizontally scrollable result tables.
- Tablet layouts (641–1060 CSS px): narrowed sidebar, adaptive table layouts,
  responsive management tabs and split editor columns.
- Laptop and desktop: full sidebar with flexible workspace widths.
- PWA: `manifest.webmanifest`, standalone display mode, shortcuts, SVG icon,
  client-side navigation and a service worker.

Only the **static application shell** (HTML, CSS, JS, icon and manifest) may
be cached for offline startup. API responses, indexer results, passwords,
admin tokens, search history, OAuth codes and emails are **never written**
into the service-worker cache. Search and authentication need an online
connection, so offline mode displays a clear status rather than fake data.

Installation requires HTTPS except on localhost. On Chrome/Edge, use
**Install app** when offered. On Safari for iOS/iPadOS, use Share > Add to
Home Screen; Safari does not offer the Chromium install prompt.
Web push notifications are not yet implemented; email delivery is separately
configurable on the server.

The appearance system (six themes, per-token colors, layout order, custom
CSS) works in the installed app and remains local to that browser profile.
