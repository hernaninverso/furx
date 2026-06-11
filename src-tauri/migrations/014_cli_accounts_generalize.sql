-- B9.1 · Generalizar claude_accounts a cli_accounts.
-- HIGH fix (Gemini + Codex audit B9.1): SQLite no soporta "ADD COLUMN IF NOT EXISTS",
-- pero rusqlite_migration garantiza ejecución unica por version. Si una DB queda en estado
-- raro (manual ALTER previo), las migraciones bombean error claro y el user resuelve manual.
-- Para defensa extra, usamos rusqlite_migration's tracking de versiones — esta migración
-- se aplica exactamente una vez por DB.

ALTER TABLE claude_accounts ADD COLUMN cli_kind TEXT NOT NULL DEFAULT 'claude';
ALTER TABLE claude_accounts ADD COLUMN env_var TEXT;
ALTER TABLE claude_accounts ADD COLUMN keychain_service TEXT;

UPDATE claude_accounts
SET env_var = 'CLAUDE_CODE_OAUTH_TOKEN',
    keychain_service = 'claude-max-' || slug
WHERE cli_kind = 'claude' AND env_var IS NULL;

CREATE VIEW IF NOT EXISTS cli_accounts AS SELECT
  cli_kind, slug, label, browser, status, env_var, keychain_service,
  last_verified_at, last_used_at, created_at, updated_at
FROM claude_accounts;

CREATE INDEX IF NOT EXISTS idx_claude_accounts_kind ON claude_accounts(cli_kind);
