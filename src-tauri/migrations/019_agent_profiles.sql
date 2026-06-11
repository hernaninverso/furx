-- 006-agent-profiles — el agente como entidad de primera clase.
-- Config NO-secreta sólo. Los secrets/tokens viven en Keychain (F-I BYOK); aquí sólo
-- se referencia la cuenta vía account_slug (NUNCA el token). El runtime resuelve el
-- secret desde Keychain justo antes de exec.

CREATE TABLE IF NOT EXISTS agent_profiles (
    id              TEXT PRIMARY KEY,
    name            TEXT NOT NULL UNIQUE,
    description     TEXT NOT NULL DEFAULT '',
    cli_kind        TEXT NOT NULL,              -- zsh|claude|codex|gemini|aider|openai-api|custom
    account_slug    TEXT,                       -- NULL = cuenta default del CLI
    model           TEXT,                       -- NULL = default del CLI
    system_prompt   TEXT NOT NULL DEFAULT '',
    default_cwd     TEXT,
    council_enabled INTEGER NOT NULL DEFAULT 0,
    council_preset  TEXT,
    shell_enabled   INTEGER NOT NULL DEFAULT 0,
    icon            TEXT,
    color           TEXT,
    is_builtin      INTEGER NOT NULL DEFAULT 0, -- 1 = sembrado desde un mode legacy (no borrable)
    created_at      TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at      TEXT NOT NULL DEFAULT (datetime('now'))
);

-- Allow-list de plugins por agente. NO duplica los permisos del plugin (esos los gobierna
-- el manifest firmado + consent store); sólo habilita/deshabilita plugins ya instalados.
CREATE TABLE IF NOT EXISTS agent_profile_plugins (
    agent_id   TEXT NOT NULL,
    plugin_id  TEXT NOT NULL,
    enabled    INTEGER NOT NULL DEFAULT 1,
    PRIMARY KEY (agent_id, plugin_id),
    FOREIGN KEY (agent_id) REFERENCES agent_profiles(id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_agent_plugins_agent ON agent_profile_plugins(agent_id);
