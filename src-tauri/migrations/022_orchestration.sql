-- 008-parallel-orchestration — orquestación de N tareas en worktrees aislados.
-- Council 2026-05-29: completion = PTY exit + "mark ready" explícito (NO polling/timeout);
-- branch única por tarea; cada tarea corre detached (tmux), el pane es vista on-demand.

CREATE TABLE IF NOT EXISTS orchestration_batches (
    id          TEXT PRIMARY KEY,
    title       TEXT NOT NULL DEFAULT '',
    repo_path   TEXT NOT NULL,
    base_branch TEXT,
    base_commit TEXT,
    created_at  TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS orchestration_tasks (
    id               TEXT PRIMARY KEY,
    batch_id         TEXT NOT NULL,
    title            TEXT NOT NULL,
    objective        TEXT NOT NULL DEFAULT '',     -- prompt/objetivo del agente
    agent_profile_id TEXT,                          -- agente (006); NULL = mode legacy
    mode             TEXT,                          -- fallback si no hay agent_profile_id
    repo_path        TEXT NOT NULL,
    branch           TEXT NOT NULL,                 -- branch única (furx/orch/<batch>/<task>)
    worktree_path    TEXT,                          -- se completa al crear el worktree
    pane_id          TEXT,                          -- pane montado (on-demand); NULL = detached
    -- pending → running → awaiting_review → done | failed | canceled
    state            TEXT NOT NULL DEFAULT 'pending'
                       CHECK (state IN ('pending','running','awaiting_review','done','failed','canceled')),
    exit_code        INTEGER,
    result_summary   TEXT,                          -- git diff --stat al recolectar
    created_at       TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at       TEXT NOT NULL DEFAULT (datetime('now')),
    UNIQUE (repo_path, branch),   -- branch única real por repo (audit codex+deepseek)
    FOREIGN KEY (batch_id) REFERENCES orchestration_batches(id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_orch_tasks_batch ON orchestration_tasks(batch_id);
CREATE INDEX IF NOT EXISTS idx_orch_tasks_state ON orchestration_tasks(state);
