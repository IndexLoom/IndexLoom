-- Fine-grained account roles. Existing user records gain admin privileges for
-- compatibility; the oldest account becomes the first owner.
ALTER TABLE auth_users
    ADD COLUMN role TEXT NOT NULL DEFAULT 'admin'
        CHECK (role IN ('owner', 'admin', 'viewer'));

UPDATE auth_users SET role = 'owner'
WHERE id = (SELECT id FROM auth_users ORDER BY created_at ASC, id ASC LIMIT 1);

-- Defense in depth: no concurrent request may remove the last active owner.
CREATE TRIGGER auth_protect_last_owner_update
BEFORE UPDATE OF role, disabled ON auth_users
WHEN OLD.role = 'owner' AND OLD.disabled = 0
 AND (NEW.role <> 'owner' OR NEW.disabled <> 0)
 AND (SELECT COUNT(*) FROM auth_users WHERE role = 'owner' AND disabled = 0) < 2
BEGIN
    SELECT RAISE(ABORT, 'cannot remove the last active owner');
END;

CREATE TRIGGER auth_protect_last_owner_delete
BEFORE DELETE ON auth_users
WHEN OLD.role = 'owner' AND OLD.disabled = 0
 AND (SELECT COUNT(*) FROM auth_users WHERE role = 'owner' AND disabled = 0) < 2
BEGIN
    SELECT RAISE(ABORT, 'cannot delete the last active owner');
END;

CREATE INDEX auth_users_role_idx ON auth_users(role, disabled);

-- Minimal security audit metadata, no passwords or raw bearer tokens.
CREATE TABLE auth_audit_events (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    occurred_at INTEGER NOT NULL,
    actor_id TEXT,
    subject_id TEXT,
    action TEXT NOT NULL CHECK (length(action) BETWEEN 3 AND 64),
    detail TEXT NOT NULL DEFAULT ''
);
CREATE INDEX auth_audit_recent_idx ON auth_audit_events(occurred_at DESC);
