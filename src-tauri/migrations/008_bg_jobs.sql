-- 2.8 — Background agent queue.
-- kinds permitidas: pr_description, standup, explain, council_review, embeddings_index, replay_bundle, eval_run.

CREATE TABLE IF NOT EXISTS bg_jobs (
    id              TEXT PRIMARY KEY,
    kind            TEXT NOT NULL,
    args_json       TEXT NOT NULL,
    status          TEXT NOT NULL CHECK (status IN ('pending','running','done','error','cancelled')),
    created_at      TEXT NOT NULL DEFAULT (datetime('now')),
    started_at      TEXT,
    finished_at     TEXT,
    output          TEXT,
    error           TEXT
);
CREATE INDEX IF NOT EXISTS idx_bg_jobs_status_created ON bg_jobs(status, created_at);

-- 2.1 — Search embeddings index.
CREATE TABLE IF NOT EXISTS search_embeddings (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    project_path    TEXT NOT NULL,
    file_path       TEXT NOT NULL,
    chunk_id        INTEGER NOT NULL,
    chunk_text      TEXT NOT NULL,
    chunk_hash      TEXT NOT NULL,
    embedding       BLOB NOT NULL,
    indexed_at      TEXT NOT NULL DEFAULT (datetime('now')),
    UNIQUE (project_path, file_path, chunk_id)
);
CREATE INDEX IF NOT EXISTS idx_search_emb_project ON search_embeddings(project_path);
CREATE INDEX IF NOT EXISTS idx_search_emb_hash ON search_embeddings(chunk_hash);
