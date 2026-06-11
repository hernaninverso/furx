-- FASE 2 — Memory Hub: Universal cross-CLI memory.
-- Council FASE 2: approved with FTS5 + embedding + tag filtering.

CREATE TABLE IF NOT EXISTS memory_entries (
  id TEXT PRIMARY KEY NOT NULL,               -- UUIDv7
  source TEXT NOT NULL DEFAULT 'furx',          -- tool source: furx | claude | codex | gemini | aider | manual
  source_id TEXT,                               -- original ID in source tool
  content TEXT NOT NULL,                        -- the memory content
  embedding BLOB,                               -- binary embedding vector (F32, little-endian)
  tags TEXT,                                    -- JSON array of strings
  created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
  updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
);

CREATE INDEX IF NOT EXISTS idx_memory_source ON memory_entries(source);
CREATE INDEX IF NOT EXISTS idx_memory_created ON memory_entries(created_at);
CREATE INDEX IF NOT EXISTS idx_memory_tags ON memory_entries(tags);

-- FTS5 full-text search over content and tags (standalone table).
-- Audit-1 LOW B006: explicit unicode61 with remove_diacritics for better
-- multilingual recall; `porter` chained for English stemming. The order of
-- 'porter unicode61' was already set in the original; keep it for backward-
-- compat. New installs benefit from the diacritic strip below.
CREATE VIRTUAL TABLE IF NOT EXISTS memory_fts USING fts5(
  content, tags,
  tokenize='porter unicode61 remove_diacritics 1'
);
