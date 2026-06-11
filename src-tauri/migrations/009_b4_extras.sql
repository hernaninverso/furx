-- B4 — gap MED: snippets, pane_templates, project_themes, time_tracking,
-- pane_env, quick_notes, http_history, sql_history, bisect_runs.

CREATE TABLE IF NOT EXISTS snippets (
    id TEXT PRIMARY KEY,
    title TEXT NOT NULL,
    body TEXT NOT NULL,
    tags TEXT NOT NULL DEFAULT '',
    source TEXT NOT NULL DEFAULT 'manual',
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);
CREATE INDEX IF NOT EXISTS idx_snippets_created ON snippets(created_at DESC);

CREATE TABLE IF NOT EXISTS pane_templates (
    name TEXT PRIMARY KEY,
    mode TEXT NOT NULL,
    cwd TEXT,
    env_keys TEXT NOT NULL DEFAULT '[]',
    initial_prompt TEXT,
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS project_themes (
    project TEXT PRIMARY KEY,
    accent_hex TEXT NOT NULL DEFAULT '#2bd1ea',
    label TEXT,
    updated_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS pane_env (
    pane_id TEXT NOT NULL,
    key TEXT NOT NULL,
    keychain_service TEXT NOT NULL,
    PRIMARY KEY (pane_id, key)
);

CREATE TABLE IF NOT EXISTS quick_notes (
    id TEXT PRIMARY KEY,
    body TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);
CREATE INDEX IF NOT EXISTS idx_quick_notes_created ON quick_notes(created_at DESC);

CREATE TABLE IF NOT EXISTS http_history (
    id TEXT PRIMARY KEY,
    method TEXT NOT NULL,
    url TEXT NOT NULL,
    status INTEGER,
    elapsed_ms INTEGER,
    bytes INTEGER,
    at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS sql_history (
    id TEXT PRIMARY KEY,
    db_alias TEXT NOT NULL,
    query TEXT NOT NULL,
    rows_returned INTEGER,
    elapsed_ms INTEGER,
    at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS bisect_runs (
    id TEXT PRIMARY KEY,
    repo_path TEXT NOT NULL,
    good TEXT NOT NULL,
    bad TEXT NOT NULL,
    test_cmd TEXT NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('pending','running','done','error','cancelled')),
    result_sha TEXT,
    output TEXT,
    at TEXT NOT NULL DEFAULT (datetime('now'))
);
