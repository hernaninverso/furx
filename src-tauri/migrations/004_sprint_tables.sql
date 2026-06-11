-- Sprint 2026-05-25 — F1/F2/F11/F21/F25 backing tables.
-- Idempotente: CREATE IF NOT EXISTS. ALTER TABLE protected con check defensivo en runtime.

-- F25 — workspace snapshots (manual ⌘⇧S + auto cada 100 events importantes).
CREATE TABLE IF NOT EXISTS snapshots (
    id              TEXT PRIMARY KEY,
    at              TEXT NOT NULL DEFAULT (datetime('now')),
    kind            TEXT NOT NULL CHECK (kind IN ('manual','auto','startup')),
    payload         TEXT NOT NULL,
    schema_version  INTEGER NOT NULL DEFAULT 1
);

CREATE INDEX IF NOT EXISTS idx_snapshots_at ON snapshots(at DESC);

-- F11 — project registry cache (escaneo de ~/, dirs con .git).
CREATE TABLE IF NOT EXISTS projects (
    path            TEXT PRIMARY KEY,
    name            TEXT NOT NULL,
    branch          TEXT,
    last_commit     TEXT,
    last_commit_at  TEXT,
    dirty           INTEGER NOT NULL DEFAULT 0,
    scanned_at      TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE INDEX IF NOT EXISTS idx_projects_scanned ON projects(scanned_at DESC);
