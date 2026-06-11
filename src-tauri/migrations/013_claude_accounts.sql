-- B9 · Claude Accounts management
-- Persiste slugs de cuentas Claude Max conectadas. El TOKEN nunca está acá —
-- solo el slug (ej "A", "B", "work") + label visible. El token vive en macOS Keychain
-- bajo el entry `claude-max-<slug>` (ver ~/bin/setup-max-account.sh).

CREATE TABLE IF NOT EXISTS claude_accounts (
  slug TEXT PRIMARY KEY NOT NULL,         -- [A-Za-z0-9_-]{1,32}, ej "A", "B", "work", "personal"
  label TEXT NOT NULL,                    -- nombre visible en UI, ej "el autor personal", "Inverso work"
  browser TEXT,                           -- "Chrome" | "Firefox" | "Safari" | "Brave" | "Arc" | "Edge" | NULL
  status TEXT NOT NULL DEFAULT 'unverified', -- "verified" | "unverified" | "missing_token"
  last_verified_at TEXT,                  -- ISO-8601 último ping ok
  last_used_at TEXT,                      -- ISO-8601 última vez que un pane lo usó
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_claude_accounts_status ON claude_accounts(status);
