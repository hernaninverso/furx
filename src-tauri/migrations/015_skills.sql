-- FASE 1 — Skills Registry
-- Tablas para skills instalables y su historial de ejecución.
-- Council FASE 1: aprobado con UUIDv7 + composite name+version + FK.

CREATE TABLE IF NOT EXISTS skills (
  id TEXT PRIMARY KEY NOT NULL,               -- UUIDv7
  name TEXT NOT NULL,
  version TEXT NOT NULL DEFAULT '1.0.0',
  description TEXT,
  category TEXT NOT NULL DEFAULT 'general',
  routing_config TEXT,                         -- JSON: {strategy, preset, max_tokens, fallback, retry}
  tools_config TEXT,                           -- JSON: [{name, endpoint, enabled}]
  permissions TEXT,                            -- JSON: [network:api, read:files, ...]
  enabled INTEGER NOT NULL DEFAULT 1,
  installed_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
  updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
  UNIQUE(name, version)
);

CREATE INDEX IF NOT EXISTS idx_skills_name ON skills(name);
CREATE INDEX IF NOT EXISTS idx_skills_category ON skills(category);
CREATE INDEX IF NOT EXISTS idx_skills_enabled ON skills(enabled);

CREATE TABLE IF NOT EXISTS skill_runs (
  id TEXT PRIMARY KEY NOT NULL,               -- UUIDv7
  skill_name TEXT NOT NULL,
  skill_version TEXT,
  input_hash TEXT NOT NULL,
  input TEXT,
  output TEXT,
  model_used TEXT,
  tokens_used INTEGER DEFAULT 0,
  latency_ms INTEGER DEFAULT 0,
  status TEXT NOT NULL DEFAULT 'pending',      -- pending | running | success | error | cancelled
  error TEXT,
  started_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
  finished_at TEXT,
  FOREIGN KEY (skill_name) REFERENCES skills(name)
);

CREATE INDEX IF NOT EXISTS idx_skill_runs_skill ON skill_runs(skill_name);
CREATE INDEX IF NOT EXISTS idx_skill_runs_status ON skill_runs(status);
CREATE INDEX IF NOT EXISTS idx_skill_runs_input ON skill_runs(input_hash);
CREATE INDEX IF NOT EXISTS idx_skill_runs_started ON skill_runs(started_at);
