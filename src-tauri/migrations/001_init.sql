-- Schema base — Fase 0 scaffold. Fases siguientes extienden (no rompen).

CREATE TABLE IF NOT EXISTS panes (
    id TEXT PRIMARY KEY,
    layout_pos INTEGER NOT NULL,
    mode TEXT NOT NULL,
    cwd TEXT,
    title TEXT,
    state TEXT NOT NULL DEFAULT 'idle',
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS cards (
    id TEXT PRIMARY KEY,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    project TEXT NOT NULL,
    source TEXT NOT NULL,
    title TEXT NOT NULL,
    cause TEXT,
    severity TEXT NOT NULL CHECK (severity IN ('info','warning','critical')),
    blast_radius TEXT,
    confidence REAL,
    deadline TEXT,
    status TEXT NOT NULL DEFAULT 'open' CHECK (status IN ('open','closed')),
    decision TEXT NOT NULL DEFAULT '',
    decided_at TEXT,
    decision_note TEXT,
    payload TEXT
);

CREATE INDEX IF NOT EXISTS idx_cards_status_created ON cards(status, created_at DESC);

-- Append-only audit. Triggers block UPDATE/DELETE.
CREATE TABLE IF NOT EXISTS events (
    id TEXT PRIMARY KEY,
    at TEXT NOT NULL DEFAULT (datetime('now')),
    kind TEXT NOT NULL,
    actor TEXT NOT NULL,
    pane_id TEXT,
    card_id TEXT,
    correlation_id TEXT,
    payload TEXT NOT NULL DEFAULT '{}'
);

CREATE INDEX IF NOT EXISTS idx_events_at ON events(at DESC);
CREATE INDEX IF NOT EXISTS idx_events_corr ON events(correlation_id) WHERE correlation_id IS NOT NULL;

CREATE TRIGGER IF NOT EXISTS events_no_update
BEFORE UPDATE ON events
BEGIN
    SELECT RAISE(ABORT, 'events table is append-only');
END;

CREATE TRIGGER IF NOT EXISTS events_no_delete
BEFORE DELETE ON events
BEGIN
    SELECT RAISE(ABORT, 'events table is append-only');
END;
