-- 014-orchestration-ux — patrones de UX de orquestación duplicados (diseño propio).
-- Cubre:
--   FR-001 best-of-N grouping (variante/grupo: tareas que son variantes del mismo objetivo)
--   FR-003 log-history por tarea (persistir el scrollback PTY snapshot, reusa 008/012)
--   FR-005 lock registry por archivo/recurso (puertos/DB dev compartidos entre worktrees)
-- SQLite no soporta ADD COLUMN IF NOT EXISTS; rusqlite_migration garantiza idempotencia via
-- user_version (no se re-corre esta migración).

-- ── FR-001 best-of-N grouping ────────────────────────────────────────────────
-- Un grupo = N variantes (≤4) de un mismo objetivo, cada una en su worktree/branch (008).
-- chosen_task_id se setea cuando el humano elige una para mergear; el resto se descarta
-- (con confirmación — constitución VI).
CREATE TABLE IF NOT EXISTS orch_task_groups (
    id              TEXT PRIMARY KEY,
    batch_id        TEXT NOT NULL,
    objective       TEXT NOT NULL DEFAULT '',  -- objetivo común de las variantes
    n               INTEGER NOT NULL,          -- nº de variantes (≤4)
    chosen_task_id  TEXT,                       -- variante elegida (NULL = sin decidir)
    created_at      TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at      TEXT NOT NULL DEFAULT (datetime('now')),
    FOREIGN KEY (batch_id) REFERENCES orchestration_batches(id) ON DELETE CASCADE
);

-- Relación variante→grupo en la tarea. group_id NULL = tarea normal (no best-of-N).
-- variant_index = posición de la variante en el grupo (0..n-1) para etiquetar la card.
ALTER TABLE orchestration_tasks ADD COLUMN group_id TEXT;
ALTER TABLE orchestration_tasks ADD COLUMN variant_index INTEGER;
CREATE INDEX IF NOT EXISTS idx_orch_tasks_group ON orchestration_tasks(group_id);

-- ── FR-003 log-history por tarea ─────────────────────────────────────────────
-- Snapshots del buffer-tail de la pane (ANSI-stripped) capturados por el poller (012) +
-- en transiciones (mark-ready). Append-only; cap/rotación por tarea (edge-case spec).
CREATE TABLE IF NOT EXISTS orch_log_history (
    id          TEXT PRIMARY KEY,
    task_id     TEXT NOT NULL,
    captured_at TEXT NOT NULL DEFAULT (datetime('now')),
    source      TEXT NOT NULL DEFAULT 'poller',  -- poller | mark_ready | manual
    content     TEXT NOT NULL,                    -- líneas join('\n'), ANSI-stripped
    FOREIGN KEY (task_id) REFERENCES orchestration_tasks(id) ON DELETE CASCADE
);
CREATE INDEX IF NOT EXISTS idx_orch_log_history_task ON orch_log_history(task_id, captured_at);

-- ── FR-005 lock registry por archivo/recurso ─────────────────────────────────
-- Coordinación más allá del aislamiento por worktree: puertos/DB de dev, o la fase
-- `git worktree add` serializada por repo. owner_task_id NULL = lock libre (releaseado).
-- resource_key es opaco (ej "port:3000", "repo-wt-add:/Users/dev/furx", "devdb:furx").
CREATE TABLE IF NOT EXISTS orch_resource_locks (
    resource_key   TEXT PRIMARY KEY,
    owner_task_id  TEXT,                          -- tarea dueña (NULL = libre)
    acquired_at    TEXT NOT NULL DEFAULT (datetime('now')),
    expires_at     TEXT,                          -- TTL opcional (GC de locks colgados)
    note           TEXT
);
CREATE INDEX IF NOT EXISTS idx_orch_locks_owner ON orch_resource_locks(owner_task_id);
