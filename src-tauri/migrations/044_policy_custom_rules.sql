-- 027 F2-wiring · storage de reglas de política custom (hardening-only) + audit de cambios.
--
-- El motor `services/policy.rs` (F0/F1/F2) ya evalúa reglas custom en memoria; esta migración les da
-- PERSISTENCIA local + un audit append-only de los CAMBIOS de política (council §"auditar los cambios
-- de política", no sólo las decisiones).
--
-- INVARIANTES:
--   - `policy_rules` es la política VIGENTE: MUTABLE (el admin agrega/edita/borra reglas). `id` UNIQUE
--     (council: la unicidad la garantiza el storage, no el motor puro).
--   - `policy_rule_changes` es APPEND-ONLY (triggers BEFORE UPDATE/DELETE → RAISE(ABORT), igual que
--     034/043): cada alta/edición/baja de una regla deja un registro inmutable con el snapshot.
--   - hardening-only: la columna `decision` NUNCA es 'allow' (lo valida el backend `is_valid_hardening`
--     antes de insertar; el CHECK acá es defensa en profundidad).
--   - Default OFF: la feature no aplica reglas custom salvo que `policy.custom_enabled` = true (setting).

-- ── Reglas custom vigentes (mutable) ───────────────────────────────────────────
CREATE TABLE IF NOT EXISTS policy_rules (
    id                  TEXT PRIMARY KEY,                  -- id estable de la regla (UNIQUE por PK)
    description         TEXT NOT NULL DEFAULT '',
    match_command       TEXT,                              -- NULL = comodín
    match_risk          TEXT,                              -- 'safe'|'destructive'|'credential'|'external'|NULL
    match_agent_profile TEXT,                              -- NULL = comodín
    match_plugin        TEXT,                              -- NULL = comodín
    decision            TEXT NOT NULL,                     -- 'deny'|'require_approval'|'require_n_approvals:N'
    enabled             INTEGER NOT NULL DEFAULT 1,        -- 1 = activa
    created_at          TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at          TEXT NOT NULL DEFAULT (datetime('now')),
    -- Defensa en profundidad: una regla custom JAMÁS puede ser 'allow' (relajaría el gate).
    CHECK (decision <> 'allow'),
    -- Al menos un matcher seteado (una regla sin matchers aplicaría a TODO = casi seguro un error).
    CHECK (match_command IS NOT NULL OR match_risk IS NOT NULL
           OR match_agent_profile IS NOT NULL OR match_plugin IS NOT NULL)
);

-- ── Audit append-only de los cambios de política ───────────────────────────────
CREATE TABLE IF NOT EXISTS policy_rule_changes (
    id          TEXT PRIMARY KEY,                          -- uuid del evento
    rule_id     TEXT NOT NULL,
    action      TEXT NOT NULL,                             -- 'create'|'update'|'delete'|'enable'|'disable'
    snapshot    TEXT NOT NULL,                             -- JSON de la regla al momento del cambio
    changed_at  TEXT NOT NULL DEFAULT (datetime('now'))
);
CREATE INDEX IF NOT EXISTS idx_policy_changes_rule ON policy_rule_changes(rule_id);

-- Append-only: prohibir UPDATE/DELETE sobre el audit de cambios (inmutable).
CREATE TRIGGER IF NOT EXISTS policy_rule_changes_no_update
BEFORE UPDATE ON policy_rule_changes
BEGIN
    SELECT RAISE(ABORT, 'policy_rule_changes es append-only: UPDATE prohibido');
END;

CREATE TRIGGER IF NOT EXISTS policy_rule_changes_no_delete
BEFORE DELETE ON policy_rule_changes
BEGIN
    SELECT RAISE(ABORT, 'policy_rule_changes es append-only: DELETE prohibido');
END;
