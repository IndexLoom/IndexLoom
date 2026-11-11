-- Track catalog membership separately from each user's enabled preference.
-- Retaining retired definitions preserves stored secrets, groups and routing.
ALTER TABLE indexer_definitions ADD COLUMN catalog_available INTEGER
    NOT NULL DEFAULT 1 CHECK (catalog_available IN (0, 1));
ALTER TABLE indexer_definitions ADD COLUMN catalog_generation TEXT;

-- Pre-migration source definitions become managed by the first new catalog sync.
-- Custom definitions added directly to storage remain outside catalog retirement.
UPDATE indexer_definitions SET catalog_generation = 'legacy'
WHERE source IN ('jackett', 'prowlarr-indexers');

CREATE INDEX catalog_available_idx
    ON indexer_definitions(catalog_available, catalog_generation);
