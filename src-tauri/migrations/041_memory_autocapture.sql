-- spec-023 F0 — Memoria auto-captura across-CLIs + Memory Hub no-opaco.
--
-- Migración NO-DESTRUCTIVA. Tres bloques:
--   (1) Procedencia + gobierno en `memory_entries` (rationale, kind, cli_kind, session_id).
--       `source`, `source_id` y `project_key` YA existen (016/021); `source_id` cumple de pane_id.
--       Backfill seguro por DEFAULT: las entries previas quedan con kind='episodic', rationale=NULL.
--   (2) Tabla propia `memory_proposals` — bandeja de revisión humana. NO se reusa `cards`:
--       las propuestas necesitan procedencia fina, confidence, dos hashes y la transición
--       INMUTABLE a `memory_entries` al aceptar. Status: proposed|accepting|accepted|rejected|edited.
--       `accepting` es un estado TRANSIENTE de claim atómico (audit MED — TOCTOU): el accept lo
--       reclama `proposed→accepting` antes de crear el entry, garantizando 1 entry por id ante
--       accepts concurrentes; si el insert falla, vuelve a `proposed` (reintentable).
--   (3) `incomplete_sessions` — resguardo TTL 5 min del SessionBuffer (ya scrubeado) ante cierre
--       ABRUPTO del pane (kill/cancelación de usuario); `cancel_reap_emit` lo guarda antes de matar
--       el pane y `run_capture` lo reprocesa (destila + purga TTL) al próximo idle/capture.
--
-- Council v2 (MoA APRUEBA-CON-CAMBIOS): ver specs/023-memory-autocapture/clarify.md.

-- (1) memory_entries: procedencia + kind + rationale (no-destructivo).
ALTER TABLE memory_entries ADD COLUMN rationale TEXT;
ALTER TABLE memory_entries ADD COLUMN kind TEXT NOT NULL DEFAULT 'episodic';
ALTER TABLE memory_entries ADD COLUMN cli_kind TEXT;
ALTER TABLE memory_entries ADD COLUMN session_id TEXT;

-- (2) Bandeja de propuestas (tabla propia). content YA viene scrubeado.
CREATE TABLE IF NOT EXISTS memory_proposals (
  id              TEXT PRIMARY KEY NOT NULL,
  project_key     TEXT NOT NULL DEFAULT '__global__',
  source          TEXT NOT NULL DEFAULT 'autocapture',  -- cli_kind del pane de origen
  source_id       TEXT,                                  -- pane_id
  cli_kind        TEXT,
  session_id      TEXT,
  content         TEXT NOT NULL,                         -- ya saneado por cloud_sanitizer
  kind            TEXT,                                  -- inferido por el LLM (nullable)
  confidence_score REAL,
  status          TEXT NOT NULL DEFAULT 'proposed',      -- proposed|accepting|accepted|rejected|edited
  rationale       TEXT,
  created_at      TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
  decided_at      TEXT,
  hash_original   TEXT,                                  -- hash del scrollback saneado de origen (dedup por sesión)
  hash_sanitized  TEXT                                   -- hash del content saneado (dedup de candidata)
);

CREATE INDEX IF NOT EXISTS idx_memory_proposals_status
  ON memory_proposals(status, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_memory_proposals_dedup
  ON memory_proposals(session_id, hash_sanitized);

-- (3) Resguardo del SessionBuffer ante cierre abrupto (TTL 5 min). content saneado.
CREATE TABLE IF NOT EXISTS incomplete_sessions (
  id           TEXT PRIMARY KEY NOT NULL,
  pane_id      TEXT,
  cli_kind     TEXT,
  project_key  TEXT NOT NULL DEFAULT '__global__',
  session_id   TEXT,
  content      TEXT NOT NULL,                            -- buffer saneado unido
  created_at   TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
  expires_at   TEXT NOT NULL                             -- created_at + 5 min (ISO UTC)
);

CREATE INDEX IF NOT EXISTS idx_incomplete_sessions_expires
  ON incomplete_sessions(expires_at);
