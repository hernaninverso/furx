-- 019 F? · T024 — retención + exportabilidad del audit del flujo review (cierra el MUST de FR-005).
--
-- DISEÑO (council 3-frontera, Opción B unánime): EXPORT-THEN-ROTATE, SIN DELETE físico.
-- El audit vive en `events` (migración 001) + `review_audit_links` (migración 034), ambas
-- APPEND-ONLY e INMUTABLES (triggers BEFORE UPDATE/DELETE → RAISE(ABORT)). Esta migración NO toca
-- esos triggers ni borra filas: la retención se hace por EXPORT a archivo + rotación a nivel
-- archivo externo. La "purga" se audita como un evento NUEVO (`audit.rotation.completed`), nunca
-- como mutación del histórico.
--
-- `audit_export_manifests` es el LIBRO SELLADO de cada export/rotación: registra qué se exportó,
-- cuántas filas, el hash del contenido (para verificar round-trip) y dónde quedó el archivo. Es
-- APPEND-ONLY igual que las tablas que sella (mismos triggers no_update/no_delete que 034): un
-- manifest no se reescribe ni se borra — sería romper la trazabilidad de la propia retención.

CREATE TABLE IF NOT EXISTS audit_export_manifests (
    manifest_id    TEXT PRIMARY KEY,                 -- UUID v4 del export/rotación
    kind           TEXT NOT NULL,                    -- export | rotation
    cutoff_ts      TEXT,                             -- corte de la ventana (NULL si export-all)
    row_count      INTEGER NOT NULL,                 -- nº de filas de audit selladas en el archivo
    content_sha256 TEXT NOT NULL,                    -- sha256 hex del contenido exportado (round-trip)
    path           TEXT NOT NULL,                    -- ruta del archivo escrito
    format         TEXT NOT NULL,                    -- ndjson | csv
    high_water     TEXT,                             -- max(event_id) del snapshot sellado (HWM, prueba qué se cerró; NULL si vacío)
    created_at     TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE INDEX IF NOT EXISTS idx_audit_export_created ON audit_export_manifests(created_at);

-- Append-only: el manifest de retención es tan inmutable como el audit que sella.
CREATE TRIGGER IF NOT EXISTS audit_export_manifests_no_update
BEFORE UPDATE ON audit_export_manifests
BEGIN
    SELECT RAISE(ABORT, 'audit_export_manifests is append-only');
END;

CREATE TRIGGER IF NOT EXISTS audit_export_manifests_no_delete
BEFORE DELETE ON audit_export_manifests
BEGIN
    SELECT RAISE(ABORT, 'audit_export_manifests is append-only');
END;
