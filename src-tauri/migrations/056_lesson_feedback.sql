-- spec-050 · Ola 8 P2 (FR-002) — Gotcha feedback loop UI.
--
-- Cierra el loop de auto-aprendizaje de gotchas procedurales (025/042): hoy el sistema CAPTURA
-- pares fallo→fix y, tras aprobación humana, los inyecta como lecciones — pero NO había superficie
-- para que el usuario diga "¿este gotcha fue útil?". Esta tabla registra ese feedback POR lección
-- aprobada (memory_entries kind=procedural), agregando votos útil / no-útil.
--
-- DECISIÓN HUMANA (foco humano 030-034): el feedback es ADVISORY. NUNCA auto-desactiva ni auto-borra
-- una lección — solo informa al usuario (que decide con el toggle existente `lesson_set_active`). Es
-- observacional: cero efecto automático sobre la inyección.
--
-- Coordinación de migración: 053 = cost-router F1; 054 reservado a cost-router F2; 055 = reliability
-- (esta Ola 8 P2 FR-003). Esta (FR-002) usa 056. ADITIVA → cero regresión.

CREATE TABLE IF NOT EXISTS lesson_feedback (
  entry_id      TEXT NOT NULL,                       -- = memory_entries.id (kind=procedural)
  project_key   TEXT NOT NULL DEFAULT '__global__',
  useful_count      INTEGER NOT NULL DEFAULT 0,      -- votos "fue útil"
  not_useful_count  INTEGER NOT NULL DEFAULT 0,      -- votos "no fue útil"
  -- última dirección del voto del usuario para esta lección ('useful' | 'not_useful' | NULL).
  -- Permite que el botón refleje el voto vigente y que un re-voto del mismo signo sea idempotente.
  last_vote     TEXT,
  updated_at    TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ','now')),
  PRIMARY KEY (entry_id, project_key)
);

CREATE INDEX IF NOT EXISTS idx_lesson_feedback_project
  ON lesson_feedback(project_key);
