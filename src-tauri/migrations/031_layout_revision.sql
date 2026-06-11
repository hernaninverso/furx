-- 018-fase-2-multiwindow-workspace · Phase B0 (T063) — revisión monotónica del layout.
--
-- Optimistic concurrency control: agrega una columna `revision` a `layout_config`
-- para que `save` pueda rechazar una escritura STALE (dos ventanas editando el mismo
-- workspace en paralelo NO corrompen el árbol). La revisión también viaja DENTRO del
-- json (campo `revision` de LayoutConfigV1, serde default 0 para filas v1 viejas), pero
-- la columna permite el guard sin deserializar el json en el camino caliente del UPDATE.
--
-- Aditiva: las filas existentes toman revision=0 (DEFAULT). Compatible con el schema 026.
ALTER TABLE layout_config ADD COLUMN revision INTEGER NOT NULL DEFAULT 0;
