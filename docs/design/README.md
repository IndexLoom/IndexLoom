# Product interface direction

The initial interface follows the reference in
[`indexloom-primary-screen-concept.png`](./indexloom-primary-screen-concept.png).

The concept is a direction, not product data. The implementation must not ship
fake indexers, fabricated search results, or inactive controls merely to match a
mockup. Its durable design decisions are:

- a compact navigation rail;
- search as the primary workspace;
- dense tables rather than card grids;
- a near-black/navy palette with one restrained amber accent;
- explicit indexer health and runtime status;
- a details panel only when a real result is selected.

The first scaffold implements the shell, responsive states, runtime health, and
honest empty states. Search, indexer configuration, and result details arrive as
their backend contracts become real.
