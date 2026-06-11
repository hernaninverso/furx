-- 019 F0 — review projection (capa hunk-level sobre orchestration; council opción L).
-- Estado MUTABLE de la decisión por hunk (la historia INMUTABLE de quién/cuándo/por qué la da el
-- audit append-only `events`, R2). Keyed por group_id (TaskGroup de orchestration) + hunk_id.
-- `revision` monotónica por grupo: toda decisión la incrementa; un write stale se rechaza (FR-004).

CREATE TABLE IF NOT EXISTS review_groups (
    group_id   TEXT PRIMARY KEY,        -- == orch_task_groups.id (no se duplica el lifecycle)
    revision   INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS review_hunks (
    group_id TEXT NOT NULL,
    task_id  TEXT NOT NULL,             -- == orchestration_tasks.id (la variante / OrchTask)
    hunk_id  TEXT NOT NULL,             -- estable: '{task_id}:{n}'
    file     TEXT NOT NULL,
    header   TEXT NOT NULL,
    state    TEXT NOT NULL DEFAULT 'pending' CHECK (state IN ('pending','approved','rejected')),
    PRIMARY KEY (group_id, hunk_id),
    FOREIGN KEY (group_id) REFERENCES review_groups(group_id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_review_hunks_group ON review_hunks(group_id);
CREATE INDEX IF NOT EXISTS idx_review_hunks_task  ON review_hunks(group_id, task_id);
