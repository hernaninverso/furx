-- 028 F0 · ACP Agent Registry — definiciones declarativas de agentes ACP.
--
-- Furx ya es un cliente ACP completo (services/acp.rs) y `AgentKind::Acp` es un kind de primera
-- clase, pero el binario del agente está hardcodeado (`ACP_DEFAULT_BIN = "claude-code-acp"`). Esta
-- tabla permite registrar MÚLTIPLES agentes ACP nombrados (Zed/JetBrains style) — agregar un agente
-- ACP pasa a ser DATOS, no código (diferencial agent-neutral).
--
-- INVARIANTES:
--   - `bin` se ejecuta como ARGV (nunca por shell): la validación del backend rechaza metacaracteres.
--   - SIN secretos: `env_extra` pasa el guardrail de secretos (BYOK; las keys viven en el Keychain).
--   - Cero-regresión: la semilla default (`claude-code-acp`) se inserta lazy; si no hay definiciones
--     o el id seleccionado no existe, el spawn cae a la const default. Borrar todo NO rompe el spawn.

CREATE TABLE IF NOT EXISTS acp_agents (
    id         TEXT PRIMARY KEY,                       -- id estable (slug)
    name       TEXT NOT NULL UNIQUE,                   -- nombre legible único
    bin        TEXT NOT NULL,                          -- binario ACP a spawnear (argv[0]); resoluble en PATH o ruta
    args       TEXT NOT NULL DEFAULT '[]',             -- JSON array de strings (argv[1..])
    env_extra  TEXT NOT NULL DEFAULT '{}',             -- JSON object string→string, NO-secreto (guardrail)
    enabled    INTEGER NOT NULL DEFAULT 1,
    is_default INTEGER NOT NULL DEFAULT 0,             -- 1 = la semilla default (claude-code-acp)
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now')),
    CHECK (bin <> '')
);
