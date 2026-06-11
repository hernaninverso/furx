-- 041 FR-004 — defaults patch (multi-usuario, Ola 1).
--
-- Limpia de la tabla `settings` cualquier `endpoints.*` cuyo valor apunte a INFRA DE HERNÁN, para
-- que un install existente (que ya tiene seedeado en 002 el IP/dominios del autor) caiga al default
-- localhost tras esta migración — sin que NADA del autor quede pegado en la DB del usuario.
--
-- Qué se considera "infra del autor":
--   - IP privada Tailscale `100.64.0.10` (AIE/Ollama de the dev server)
--   - dominios `example.internal` / `example.test` / `devserver.local`
--   - el repo de releases `furx-app/furx` (y cualquier repo legacy privado)
--   - el host de telemetría `telemetry.example.internal`
--
-- Diseño (corrige al consejo GTM, ver /tmp/council-gtm-result.md):
--   - NO se bloquea ningún RANGO (CGNAT 100.64/10, privados): un usuario con Tailscale debe poder
--     poner SU propio host. El candado es default-localhost + esta limpieza, no un bloqueo de rango.
--   - Comparación TIPO-SEGURA con `json_extract(value,'$')`: el valor de settings es un JSON string
--     (`'"http://..."'`), así que extraemos el string interno antes del `LIKE`. Un valor que NO sea
--     string (objeto/numero) devuelve NULL en json_extract y NO matchea → intacto.
--   - Se vacía a `json('""')` (string JSON vacío) en vez de borrar la fila: el resolver de endpoints
--     trata "" igual que ausente y cae al default localhost. Mantener la fila preserva la forma de la
--     tabla y es lo que el resolver ya espera (`.filter(|s| !s.trim().is_empty())`).
--   - IDEMPOTENTE: tras la 1ª corrida los valores ya no matchean los patrones → 2ª corrida = 0 cambios.
--   - NO toca un `endpoints.*` que apunte a la infra PROPIA del usuario (otro IP, su dominio, su repo)
--     ni a localhost/127.0.0.1: esos no matchean ningún patrón.
--
-- Nota: `rusqlite_migration` corre cada `M::up` dentro de su propia transacción, así que no hace
-- falta (ni se debe) abrir un BEGIN/COMMIT manual acá.

-- AIE / Ollama / cualquier endpoint que apunte al host privado del autor.
UPDATE settings
SET value = json('""')
WHERE key LIKE 'endpoints.%'
  AND json_type(value, '$') = 'text'
  AND json_extract(value, '$') LIKE '%100.64.0.10%';

-- Dominios de la infra del autor.
UPDATE settings
SET value = json('""')
WHERE key LIKE 'endpoints.%'
  AND json_type(value, '$') = 'text'
  AND (json_extract(value, '$') LIKE '%example.internal%'
    OR json_extract(value, '$') LIKE '%example.test%'
    OR json_extract(value, '$') LIKE '%devserver.local%');

-- Updater apuntando al repo de releases del autor.
UPDATE settings
SET value = json('""')
WHERE key = 'endpoints.updates'
  AND json_type(value, '$') = 'text'
  AND json_extract(value, '$') LIKE '%furx-legacy/private-releases%';

-- Host de telemetría del autor.
UPDATE settings
SET value = json('""')
WHERE key = 'endpoints.telemetry'
  AND json_type(value, '$') = 'text'
  AND json_extract(value, '$') LIKE '%telemetry.example.internal%';
