-- spec-050 · Ola 8 P2 (FR-001) — Multi-machine sync (metadata de last-write).
--
-- OPT-IN + FAIL-CLOSED. Sincroniza overrides MCP (`mcp_user_overrides`) + targets de monitor
-- (`monitor_targets`) + gotchas/lecciones procedurales (`memory_entries kind=procedural`) entre las
-- máquinas del MISMO usuario vía cloud relay. Tiebreaker last-write-wins `(updated_at, installation_id)`
-- — NO CRDT. Opt-in (setting `sync.multi_machine_enabled`, default OFF); si el relay falla, cada
-- máquina sigue con su estado LOCAL (cero regresión).
--
-- Esta tabla guarda, por item sincronizable, QUIÉN lo escribió por última vez (`installation_id`) y
-- CUÁNDO (`updated_at`), para desempatar merges sin tocar las tablas de origen (que no tienen columna
-- installation_id). El merge compara `(updated_at, installation_id)` lexicográficamente: gana el
-- updated_at mayor; si empatan, gana el installation_id mayor (determinista).
--
-- Coordinación de migración: 053 cost-router F1; 054 reservado a cost-router F2; 055 reliability;
-- 056 lesson_feedback; esta (FR-001) usa 057. ADITIVA → cero regresión.

CREATE TABLE IF NOT EXISTS sync_meta (
    kind            TEXT NOT NULL,                  -- 'mcp_override' | 'monitor_target' | 'lesson'
    item_id         TEXT NOT NULL,                  -- la PK lógica del item en su tabla de origen
    updated_at      TEXT NOT NULL,                  -- RFC3339-ish del último write (tiebreaker primario)
    installation_id TEXT NOT NULL,                  -- quién escribió (tiebreaker secundario)
    deleted         INTEGER NOT NULL DEFAULT 0 CHECK (deleted IN (0, 1)), -- tombstone para borrados
    PRIMARY KEY (kind, item_id)
);

CREATE INDEX IF NOT EXISTS idx_sync_meta_kind ON sync_meta(kind);
