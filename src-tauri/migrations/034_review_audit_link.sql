-- 019 F0 · T001 — audit append-only del flujo review (R2 / FR-005).
--
-- El histórico INMUTABLE de quién/qué/cuándo/por-qué lo da la tabla `events` (migración 001,
-- triggers append-only: UPDATE/DELETE → RAISE(ABORT)). Esta migración NO duplica ese histórico;
-- agrega la CAPA DE VÍNCULO explícito que pide R2: relacionar cada evento de audit del flujo
-- review (compare/approve/reject/kill/revert/apply) con el OBJETO concreto que tocó
-- (change_set == group_id, hunk_id, approval_id). Sin esto el `events.payload` lleva los ids como
-- JSON suelto y no se puede consultar "todas las decisiones del hunk X" de forma indexada.
--
-- INVARIANTES:
--   - `event_id` es FK al `events.id` ya escrito (append-only): el link NUNCA existe sin su evento.
--   - La tabla es APPEND-ONLY igual que `events` (triggers abajo): un vínculo no se reescribe ni
--     se borra. Revertir una decisión = un NUEVO evento + un NUEVO link (no muta el histórico, R2).
--   - Todos los target_* son NULLABLE: un evento `compare` referencia sólo el group; un `decide`
--     referencia group+hunk; un `apply` referencia el group; un gate referencia el approval.

CREATE TABLE IF NOT EXISTS review_audit_links (
    event_id     TEXT PRIMARY KEY,                 -- == events.id (append-only, FK lógico)
    action       TEXT NOT NULL,                    -- compare|approve|reject|revert|kill|apply
    group_id     TEXT,                             -- change-set (== orch_task_groups.id / review_groups.group_id)
    hunk_id      TEXT,                             -- hunk decidido (NULL si la acción no es por-hunk)
    approval_id  TEXT,                             -- approval consumido/creado por el gate (NULL si no aplica)
    revision     INTEGER,                          -- revisión monotónica del change-set al momento (NULL si N/A)
    actor        TEXT NOT NULL,                    -- quién (denormalizado para consulta directa)
    target       TEXT NOT NULL DEFAULT '',         -- target legible (group/hunk/worktree)
    rationale    TEXT NOT NULL DEFAULT '',         -- por qué (motivo de la acción)
    created_at   TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE INDEX IF NOT EXISTS idx_review_audit_group ON review_audit_links(group_id);
CREATE INDEX IF NOT EXISTS idx_review_audit_hunk  ON review_audit_links(hunk_id);
CREATE INDEX IF NOT EXISTS idx_review_audit_appr  ON review_audit_links(approval_id);
CREATE INDEX IF NOT EXISTS idx_review_audit_action ON review_audit_links(action);

-- Append-only: el vínculo de audit es tan inmutable como el evento que referencia (R2).
CREATE TRIGGER IF NOT EXISTS review_audit_links_no_update
BEFORE UPDATE ON review_audit_links
BEGIN
    SELECT RAISE(ABORT, 'review_audit_links is append-only');
END;

CREATE TRIGGER IF NOT EXISTS review_audit_links_no_delete
BEFORE DELETE ON review_audit_links
BEGIN
    SELECT RAISE(ABORT, 'review_audit_links is append-only');
END;
