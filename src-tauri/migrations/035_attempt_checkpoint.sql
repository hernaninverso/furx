-- 019 F0 · T005 — kill-switch transaccional con checkpoint-por-attempt (R4 / FR-006).
--
-- Protocolo (council R4): al EMPEZAR un attempt (variante del best-of-N) se registra un checkpoint
-- = el estado del worktree del que parte (HEAD del worktree + el worktree path). Un kill posterior
-- usa ESTE checkpoint para decidir cómo abortar de forma transaccional: restaurar el worktree a su
-- HEAD de checkpoint (descartando cambios sin commitear del attempt) o, si el worktree fue creado
-- por el attempt (`created == 1`), descartarlo entero. NUNCA deja el repo a medio escribir y NUNCA
-- toca procesos ajenos (el kill del PTY lo rutea el process-registry, ver orchestration_cancel).
--
-- INVARIANTE: un checkpoint por (task_id) — el attempt registra su punto de partida UNA vez
-- (idempotente, INSERT OR IGNORE). `status` marca el ciclo de vida del checkpoint:
--   open      → el attempt está vivo; el checkpoint es la base para un kill.
--   killing   → un kill GANÓ el claim y está ejecutando el git (estado RE-INTENTABLE: si el git
--               falla, vuelve a `open`; si un `killing` queda huérfano —proceso muerto a mitad—
--               se puede re-clamar tras un timeout, ver `claim_killing`).
--   killed    → el git terminó OK y el worktree quedó efectivamente limpio/descartado (consumido).
-- El kill es transaccional + idempotente (audit-3 codex/deepseek H3): el git corre PRIMERO (estado
-- `killing`); SÓLO si el worktree quedó limpio se marca `killed`. Si el git falla, el checkpoint
-- vuelve a `open` → un re-kill REINTENTA (no queda zombie DB=killed/worktree-sucio). Re-killear un
-- checkpoint ya `killed` es noop. Un `killing` huérfano (más viejo que el timeout) es re-clamable.

CREATE TABLE IF NOT EXISTS attempt_checkpoints (
    task_id        TEXT PRIMARY KEY,                -- == orchestration_tasks.id (la variante/attempt)
    group_id       TEXT,                            -- best-of-N group (NULL = attempt suelto)
    worktree_path  TEXT NOT NULL,                   -- worktree del attempt (donde escribe el agente)
    base_commit    TEXT NOT NULL,                   -- HEAD del worktree al registrar el checkpoint
    created_worktree INTEGER NOT NULL DEFAULT 0,    -- 1 = el attempt CREÓ el worktree → kill puede descartarlo
    status         TEXT NOT NULL DEFAULT 'open'
                     CHECK (status IN ('open','killing','killed')),
    created_at     TEXT NOT NULL DEFAULT (datetime('now')),
    killing_at     TEXT,                            -- cuándo un kill ganó el claim y arrancó el git (NULL = no in-flight)
    killed_at      TEXT                             -- cuándo el kill consumió el checkpoint (NULL = vivo)
);

CREATE INDEX IF NOT EXISTS idx_attempt_ckpt_group  ON attempt_checkpoints(group_id);
CREATE INDEX IF NOT EXISTS idx_attempt_ckpt_status ON attempt_checkpoints(status);
