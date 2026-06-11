-- spec-022 US1 (audit 3-frontier MED 1) — la tabla `plugins` no tenía UNIQUE(name).
-- El estado enable/disable se keyed por `name`, pero filas legacy duplicadas por name
-- (instalaciones repetidas, ON CONFLICT(id) DO NOTHING que nunca colapsaba por name)
-- hacían el SELECT … ORDER BY installed_at LIMIT 1 ambiguo y abrían una race
-- read-then-write en set_enabled. Acá: (1) colapsamos duplicados por name conservando
-- la fila más reciente (mayor installed_at, desempate por id) y su estado `enabled`;
-- (2) creamos UNIQUE(name) para que set_enabled pueda hacer un UPSERT atómico por name.

-- 1) Borrar todas las filas de un name salvo la "ganadora" (la más reciente).
DELETE FROM plugins
WHERE id NOT IN (
    SELECT id FROM (
        SELECT id,
               ROW_NUMBER() OVER (
                   PARTITION BY name
                   ORDER BY installed_at DESC, id DESC
               ) AS rn
        FROM plugins
    )
    WHERE rn = 1
);

-- 2) Índice único por name → habilita UPSERT atómico (ON CONFLICT(name)).
CREATE UNIQUE INDEX IF NOT EXISTS idx_plugins_name_unique ON plugins(name);
