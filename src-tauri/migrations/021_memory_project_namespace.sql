-- 007-cross-project-memory — namespacing por proyecto de la memoria local.
-- Council 2026-05-29: project_key TEXT (reservados __global__/__shared__), NO FK a
-- projects.path (path mutable). El ALTER con DEFAULT '__global__' backfillea las filas
-- existentes (memorias previas → globales, visibles). Capa OPCIONAL (no rompe el
-- fallback mnemo/memento — ver docs/MEMORY_INTEGRATION.md).

ALTER TABLE memory_entries ADD COLUMN project_key TEXT NOT NULL DEFAULT '__global__';
CREATE INDEX IF NOT EXISTS idx_memory_project ON memory_entries(project_key);

-- single source of truth: un proyecto puede apuntar a un meta-repo de principios.
-- Referencia (path), NO copia. Las entradas compartidas viven con project_key='__shared__'.
ALTER TABLE projects ADD COLUMN shared_source TEXT;
