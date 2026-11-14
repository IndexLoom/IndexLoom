-- Opaque durable download tickets; raw tracker URLs are encrypted with master key.
CREATE TABLE download_tickets (
  id TEXT PRIMARY KEY NOT NULL CHECK(length(id)=64),
  guid TEXT NOT NULL CHECK(length(guid)=64),
  indexer_id TEXT NOT NULL REFERENCES indexers(id) ON DELETE CASCADE,
  revision INTEGER NOT NULL,
  expires_at INTEGER NOT NULL,
  nonce BLOB NOT NULL CHECK(length(nonce)=24),
  ciphertext BLOB NOT NULL,
  created_at INTEGER NOT NULL
);
CREATE INDEX download_tickets_expiry_idx ON download_tickets(expires_at);
CREATE INDEX download_tickets_indexer_idx ON download_tickets(indexer_id);
CREATE INDEX download_tickets_guid_idx ON download_tickets(guid,created_at DESC);
