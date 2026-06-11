-- spec-025 F0/F1 — Loop de gotchas procedurales (auto-aprendizaje #1).
--
-- Migración NO-DESTRUCTIVA. Dos tablas nuevas; NO toca nada de 023 (memory_proposals/
-- memory_entries/incomplete_sessions se REUSAN tal cual).
--
--   (1) failure_signals — el lado "FALLO" del par fallo->fix. Persiste cada fallo detectado en un
--       pane de CLI de agente (verdict Failed de done_detection / marcador de error en el tail),
--       con su tail_excerpt YA SANEADO (scrub_buffer) + los artefactos (paths) detectados. El
--       correlador (procedural_gotchas.rs) empareja un failure no-resuelto con una señal de fix que
--       toque el MISMO artefacto dentro de la ventana (council v2 §1). resolved=1 al emparejar.
--   (2) lesson_activation — estado de activación POR lección aprobada (memory_entries kind=procedural)
--       para la inyección por perfil. Permite desactivar sin borrar (gobierno, council v2 §5).
--
-- Council v2 (MoA APRUEBA-CON-CAMBIOS): ver specs/025-procedural-gotchas/clarify.md.

-- (1) Señales de fallo (lado "fallo" del par). tail_excerpt y artifacts YA saneados.
CREATE TABLE IF NOT EXISTS failure_signals (
  id            TEXT PRIMARY KEY NOT NULL,
  pane_id       TEXT,
  cli_kind      TEXT,
  session_id    TEXT,
  project_key   TEXT NOT NULL DEFAULT '__global__',
  detected_at   TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
  tail_excerpt  TEXT NOT NULL,                       -- segmento del fallo, saneado por scrub_buffer
  artifacts     TEXT NOT NULL DEFAULT '[]',          -- JSON array de artefactos (paths) del fallo
  resolved      INTEGER NOT NULL DEFAULT 0           -- 1 cuando el correlador emparejó un fix
);

CREATE INDEX IF NOT EXISTS idx_failure_signals_session
  ON failure_signals(session_id);
CREATE INDEX IF NOT EXISTS idx_failure_signals_unresolved
  ON failure_signals(resolved, detected_at);

-- (2) Activación por lección aprobada (memory_entries kind=procedural). active=1 por default.
CREATE TABLE IF NOT EXISTS lesson_activation (
  entry_id     TEXT PRIMARY KEY NOT NULL,            -- = memory_entries.id (kind=procedural)
  project_key  TEXT NOT NULL DEFAULT '__global__',
  active       INTEGER NOT NULL DEFAULT 1,
  updated_at   TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
);

CREATE INDEX IF NOT EXISTS idx_lesson_activation_project
  ON lesson_activation(project_key, active);
