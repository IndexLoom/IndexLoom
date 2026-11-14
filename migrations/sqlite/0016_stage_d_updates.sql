CREATE TABLE update_cache (
    repository TEXT PRIMARY KEY NOT NULL,
    checked_at INTEGER NOT NULL,
    document TEXT NOT NULL CHECK(length(document) <= 8388608)
);
