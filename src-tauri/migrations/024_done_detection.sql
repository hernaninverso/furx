-- 012-pty-done-detection — auto-detección del ciclo de vida por polling del buffer PTY.
-- Complementa 008 (mark-ready manual SIGUE): el poller clasifica el buffer de la pane y
-- (a) auto-transiciona running→awaiting_review cuando el agente queda idle (debounce), o
-- (b) marca needs_input + emite agent.input_requested (010) ante un trust/permission prompt.
-- Auto-confirm es OPT-IN (default OFF) y con tope/min (constitución VI: no destructivo auto).

-- Flags por tarea sobre orchestration_tasks (008). SQLite no soporta ADD COLUMN IF NOT
-- EXISTS; estas columnas son nuevas en esta migración (idempotencia la da rusqlite_migration
-- vía user_version, no se re-corre).
ALTER TABLE orchestration_tasks ADD COLUMN needs_input INTEGER NOT NULL DEFAULT 0;
ALTER TABLE orchestration_tasks ADD COLUMN auto_confirm INTEGER NOT NULL DEFAULT 0;
-- cli_kind cacheado para que el classifier elija la tabla de patrones correcta sin
-- re-resolver el agent_profile en cada tick. NULL = desconocido (heurística genérica).
ALTER TABLE orchestration_tasks ADD COLUMN cli_kind TEXT;

-- Auditoría de auto-confirms: una fila por Enter auto-presionado (para el tope/min y el feed).
CREATE TABLE IF NOT EXISTS orch_auto_confirms (
    id          TEXT PRIMARY KEY,
    task_id     TEXT NOT NULL,
    confirmed_at TEXT NOT NULL DEFAULT (datetime('now')),
    matched     TEXT,                                   -- patrón que disparó el confirm
    FOREIGN KEY (task_id) REFERENCES orchestration_tasks(id) ON DELETE CASCADE
);
CREATE INDEX IF NOT EXISTS idx_orch_auto_confirms_task ON orch_auto_confirms(task_id, confirmed_at);
