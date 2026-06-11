-- Settings KV (key → JSON value). Source of truth para endpoints, keybindings, opt-ins.

CREATE TABLE IF NOT EXISTS settings (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL,
    updated_at TEXT NOT NULL DEFAULT (datetime('now'))
);

-- Layouts per workspace. `panes` = JSON array of {id, mode, title}.
-- `grid_cols` y `grid_rows` son CSS grid-template values (ej. "1fr 1fr"). Para drag-resize.
CREATE TABLE IF NOT EXISTS layouts (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    panes TEXT NOT NULL,
    grid_cols TEXT NOT NULL DEFAULT '1fr 1fr',
    grid_rows TEXT NOT NULL DEFAULT '1fr 1fr',
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now'))
);

-- Endpoints seed with safe, infra-neutral defaults. The AIE engine defaults to a
-- local instance (http://localhost:8250); telemetry and the updater are empty until
-- the user opts in / a release build configures them. Migration 048 also NULL-clears
-- any author-specific endpoints left over in pre-existing installs.
INSERT OR IGNORE INTO settings (key, value) VALUES
    ('endpoints.aie', '"http://localhost:8250"'),
    ('endpoints.telemetry', '""'),
    ('endpoints.updates', '""'),
    ('opt_in.telemetry', 'false'),
    ('opt_in.eula_accepted_at', 'null'),
    ('opt_in.crash_reports', 'false'),
    ('preferences.theme', '"dark-cyan"'),
    ('preferences.font_size', '12'),
    ('preferences.keybindings_profile', '"default"'),
    ('compat.last_check_at', 'null'),
    ('app.first_run_completed', 'false');
