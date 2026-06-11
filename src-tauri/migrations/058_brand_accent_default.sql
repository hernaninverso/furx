-- 058 (051/US1) — brand: default coral en project_themes (supersede el accent legacy '#2bd1ea'
-- de la migración 009, ya aplicada e inmutable). accent_hex SE LEE en runtime (themes.rs),
-- así que el default importa. SQLite no soporta ALTER COLUMN SET DEFAULT → rebuild de la tabla.
-- Migra además filas existentes cuyo accent sea un color de marca legacy → coral FUNCIONAL.
-- OJO: accent_hex se RENDERIZA como color funcional (texto/borde), no como logo → el default es
-- el coral ACCESIBLE #bf3f18 (AA), NO el #FF5C35 de marca (que es solo para la "F"/logo).
ALTER TABLE project_themes RENAME TO project_themes_legacy058;
CREATE TABLE project_themes (
    project    TEXT PRIMARY KEY,
    accent_hex TEXT NOT NULL DEFAULT '#bf3f18',
    label      TEXT,
    updated_at TEXT NOT NULL DEFAULT (datetime('now'))
);
INSERT INTO project_themes (project, accent_hex, label, updated_at)
SELECT project,
       CASE WHEN lower(accent_hex) IN
            ('#2bd1ea','#0d4f5c','#0d5560','#46c7c0','#0e8a96','#5ce0f0','#5ce0f1')
            THEN '#bf3f18' ELSE accent_hex END,
       label, updated_at
FROM project_themes_legacy058;
DROP TABLE project_themes_legacy058;
