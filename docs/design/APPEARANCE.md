# Appearance and full interface customization

Open **Settings** (or the Appearance link in the navigation rail). Changes
take effect immediately and persist in **this browser's localStorage** under
`indexloom.ui-preferences.v1`.

## Presets

- **Obsidian Violet:** black surfaces with violet emphasis (default).
- **Carbon:** near-black, neutral grey.
- **Midnight Blue:** restrained blue.
- **Evergreen:** dark green.
- **Burgundy:** dark rose.
- **Porcelain:** accessible high-contrast light palette.

Every color can be overridden with a hex selector: background, base surface,
elevated surface, input background, primary/secondary text, borders and accent.

The Layout tab controls navigation order, sidebar (left/right/top),
sidebar width, indexer directory/editor split, block order and visibility.
Drag blocks or use the arrow buttons for keyboard-accessible movement.

Density, corner radius (0–24px) and font scale (85–130%) are separate.
Screen-width-specific rules override incompatible sidebars on narrow devices.

## User CSS

The CSS tab provides a text editor. Its contents are applied via a `style`
element after the bundled styles and are never sent to the server.
For example:

```css
:root {
  --color-accent-solid: #ae80ff;
  --ui-radius: 6px;
}
.search-view { max-width: 1440px; }
.indexer-list-item { min-height: 38px; }
.result-indexer { border-style: solid; }
```

Useful selectors include `.app-shell`, `.sidebar`, `.navigation`,
`.workspace`, `.search-view`, `.settings-view`,
`.overview-grid`, `.search-toolbar`, `.result-indexer`,
`.management-tabs`, `.indexer-browser`, `.indexer-directory`,
`.indexer-editor-panel`, `.capability-disclosure` and
`.advanced-search`.

The full set of built-in variables is in `web/src/appearance.css`;
the editable preset colors are applied to `:root` inline. User CSS appended
after the built-in stylesheet can replace layout and styles, but inline
variables may require `!important` or scoped overrides.

CSS is intentionally unrestricted to enable complete restyling. It can hide
controls, damage legibility, or load remote resources via `url()` or
`@import`. **Import styles only from trusted sources**; reset appearance
in Settings if a design becomes unusable. A user profile cannot execute
JavaScript through the appearance loader, but CSS itself is powerful.

## Import and export

Export saves a portable `indexloom-appearance.json` file with theme,
colors, layout, hidden blocks and CSS. Import validates the JSON version,
palette keys, dimensions, permitted block/page names and CSS size
(maximum 100,000 characters) before applying it. Appearance state is
local-only; credentials and administrator tokens are never included.

The profile is a full interface style/layout configuration, not a
third-party React plugin or a sandbox for arbitrary JavaScript widgets.

## Restore defaults

Click **Reset appearance** in Settings. This restores Obsidian Violet,
all visible blocks, default order and empty custom CSS.
