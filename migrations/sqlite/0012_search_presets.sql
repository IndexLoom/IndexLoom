CREATE TABLE search_presets (
  owner TEXT NOT NULL,
  id TEXT NOT NULL CHECK(length(id) BETWEEN 1 AND 80),
  name TEXT NOT NULL CHECK(length(name) BETWEEN 1 AND 100),
  document_json TEXT NOT NULL CHECK(length(document_json) <= 32768),
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  PRIMARY KEY(owner,id)
);
CREATE INDEX search_presets_owner_idx ON search_presets(owner,updated_at DESC);
