-- BLOQUE 3 · Preset overrides + Resilience state
-- Council 5/5 unanime: skip-aie sidecar, port resilience to Rust.

-- Per-preset, per-provider toggle. NULL row = use preset_member from provider_credentials.
CREATE TABLE IF NOT EXISTS preset_overrides (
  preset TEXT NOT NULL,            -- "quick" | "cheapo" | "frontier" | "local" | "mix"
  provider_alias TEXT NOT NULL,
  enabled INTEGER NOT NULL DEFAULT 1,
  updated_at TEXT NOT NULL,
  PRIMARY KEY (preset, provider_alias)
);

-- Resilience state: tracks current rate-limit / quota / circuit-breaker per provider+model.
CREATE TABLE IF NOT EXISTS resilience_state (
  provider TEXT NOT NULL,
  model TEXT NOT NULL,
  credential_alias TEXT NOT NULL,
  dimension TEXT NOT NULL,         -- "rpm_minute" | "tpd_day" | "api_429" | "circuit"
  blocked_until TEXT,              -- ISO-8601 timestamp or NULL
  bucket_used INTEGER NOT NULL DEFAULT 0,
  bucket_limit INTEGER NOT NULL DEFAULT 0,
  bucket_window_s INTEGER NOT NULL DEFAULT 60,
  updated_at TEXT NOT NULL,
  PRIMARY KEY (provider, model, credential_alias, dimension)
);

CREATE INDEX IF NOT EXISTS idx_resilience_blocked ON resilience_state(blocked_until);
