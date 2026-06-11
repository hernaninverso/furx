-- spec-kit 046 · Ola 7 (Skills P1) — layout versionado + fast-path cache.
--
-- Construye sobre la Ola 4 (010 plugins, 039 UNIQUE(name), 049 trust columns). ADITIVA:
-- agrega una tabla satélite `skill_versions` para el update versionado (FR-001) y una
-- columna `scripts_cache_snapshot` a `plugins` para el fast-path de re-verificación
-- (FR-002). NO toca filas ni columnas existentes → cero regresión del install-only de la
-- Ola 4 (un skill instalado con el layout plano sigue funcionando: no tiene filas en
-- `skill_versions` y su `scripts_cache_snapshot` es NULL → siempre rehashea, fail-safe).

-- ── FR-001: update versionado ────────────────────────────────────────────────
-- Cada versión instalada de un skill vive en `plugins/<name>/versions/<tree_hash>/` y el
-- symlink `plugins/<name>/current` apunta a la activa. Esta tabla es la fuente de verdad
-- de QUÉ versiones existen en disco (para rollback + GC), independiente del `plugins`
-- row (que refleja la versión ACTIVA). El symlink-swap es la operación atómica; la DB
-- registra el historial.
--
-- (name, tree_hash) es único: una versión = un árbol de contenido. `version` es la
-- SemVer del SKILL.md (informativa; el tree_hash es la identidad real). `is_current=1`
-- en EXACTAMENTE una fila por name (la apuntada por el symlink). `trust_level` se copia
-- de la resolución del gate al instalar esa versión.
CREATE TABLE IF NOT EXISTS skill_versions (
    name         TEXT NOT NULL,
    tree_hash    TEXT NOT NULL,
    version      TEXT NOT NULL,
    trust_level  TEXT,                 -- 'verified'|'promoted'|'sandboxed'|'rejected'
    is_current   INTEGER NOT NULL DEFAULT 0,
    installed_at TEXT NOT NULL DEFAULT (datetime('now')),
    PRIMARY KEY (name, tree_hash)
);

-- Un solo `current` por skill: índice único parcial sobre las filas is_current=1.
CREATE UNIQUE INDEX IF NOT EXISTS idx_skill_versions_current
    ON skill_versions(name) WHERE is_current = 1;

-- ── FR-002: fast-path cache de verificación ──────────────────────────────────
-- Snapshot POR-ARCHIVO `Vec<(rel_path, inode, mtime, size)>` serializado a JSON canónico
-- y luego hasheado (SHA-256). Si el snapshot recomputado coincide con el guardado → se
-- saltea el rehash completo del contenido. Si CUALQUIER archivo cambió (mtime/size/inode)
-- o hay duda → se rehashea (fail-safe). NULL para todo plugin legacy → siempre rehashea.
ALTER TABLE plugins ADD COLUMN scripts_cache_snapshot TEXT;
