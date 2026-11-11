# IndexLoom responsive web app and PWA

The same built React application is used for desktop, laptop, tablets and
phones. Responsive CSS adapts sidebar/navigation, forms, management
panels, tables, and the theme editor. On narrow screens navigation becomes
a bottom tab bar with iPhone safe-area inset support. Tablets retain an
abbreviated side navigation with responsive management panels.

## Installing

Serve IndexLoom over **HTTPS** (localhost is accepted for development),
then use the browser's "Install app" prompt (Chrome/Edge) or **Share → Add to
Home Screen** (Safari on iOS). The web app declares a standalone display
mode, scalable SVG icon, theme color and mobile web app metadata.

A service worker precaches the empty application shell, icon and manifest.
Static built assets may be cached; paths below `/api` and all requests
with `Authorization` headers are never intercepted or cached. The service
worker cannot make remote indexers work offline. Offline account actions and
searches fail visibly, rather than returning saved private API responses.

For iOS users, the SVG icon is a portable fallback. PNG Apple touch icons
are more widely supported on older Safari versions and can be added in the
asset pipeline.

Custom themes and CSS are stored in localStorage on that particular
browser/profile, not synchronized across installations. Export and import
an appearance profile in Settings if you want the same design elsewhere.

## Testing checklist

Validate a 320px phone, 390px phone, 768px tablet, 1024px small laptop
and >=1440px desktop. Ensure keyboard, password form, email editor, search
filters, indexer tables, bottom navigation and safe areas are usable in
light and dark themes. Run the Lighthouse PWA checklist after deployment.
