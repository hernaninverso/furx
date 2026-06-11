-- 038 Goose-C P1 — ejecución del DAG de pipelines (spec 038, fase F1.0).
--
-- ADITIVA: 0 estados nuevos en la state-machine de 008; single-task sigue idéntico
-- (default `dag_blocked=0`, sin aristas → `depends_on=[]`). El DAG ya parseado/validado/
-- topo-sorteado (029 F0) se ejecuta poblando estas tablas en la MISMA transacción que
-- `create_batch`. El gate de lanzamiento es una guarda SQL (`AND dag_blocked=0`) en
-- `claim_for_launch`; "esperando deps" es DERIVADO, no un estado nuevo.

-- Un run de pipeline = un batch (022) con su grafo congelado (topo_json/yaml_sha256/spec_yaml).
CREATE TABLE IF NOT EXISTS pipeline_runs (
    id             TEXT PRIMARY KEY,
    batch_id       TEXT NOT NULL,
    name           TEXT NOT NULL DEFAULT '',
    yaml_sha256    TEXT NOT NULL DEFAULT '',
    topo_json      TEXT NOT NULL DEFAULT '[]',   -- orden topo congelado (no se re-deriva en caliente)
    spec_yaml      TEXT NOT NULL DEFAULT '',      -- YAML original (resume/auditoría)
    -- running → done | failed | canceled. NO es la state-machine de tareas (008); es el
    -- estado del RUN. El scheduler deja de promover cuando status != 'running'.
    status         TEXT NOT NULL DEFAULT 'running'
                     CHECK (status IN ('running','done','failed','canceled')),
    -- v1: HARD 1 (el scheduler promueve 1 nodo ready a la vez). v2 sube esto.
    -- CHECK > 0 (audit AIE): el scheduler asume un nº positivo de slots; 0/negativo lo colgaría.
    max_concurrent INTEGER NOT NULL DEFAULT 1 CHECK (max_concurrent > 0),
    created_at     TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at     TEXT NOT NULL DEFAULT (datetime('now')),
    -- 1:1 run↔batch (spec 038: "un run de pipeline = un batch"). UNIQUE evita dos runs sobre el
    -- mismo batch (audit deepseek) — `pipeline_run_yaml` crea batch + run juntos, nunca re-usa.
    UNIQUE (batch_id),
    FOREIGN KEY (batch_id) REFERENCES orchestration_batches(id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_pipeline_runs_status ON pipeline_runs(status);

-- Aristas del DAG normalizadas (depends_on NO es columna JSON): readiness = `NOT EXISTS`
-- una arista cuya dep no está `done`. Las FKs `ON DELETE CASCADE` evitan aristas colgantes:
-- borrar una tarea NO debe dejar una dep insatisfecha eterna (red-team riesgo #7).
CREATE TABLE IF NOT EXISTS pipeline_edges (
    run_id             TEXT NOT NULL,
    task_id            TEXT NOT NULL,   -- el dependiente (se bloquea hasta que sus deps cierren)
    depends_on_task_id TEXT NOT NULL,   -- la dep (debe llegar a `done`)
    -- 'continue' = best-effort: el fallo de la dep NO bloquea al dependiente. 'block_downstream'
    -- (default explícito) = el fallo de la dep cascadea skip a los descendientes `pending`. El
    -- scheduler (F1.3) trata `on_error != 'continue'` como bloqueante → NULL legacy y el default
    -- nuevo se comportan idéntico. DEFAULT explícito (audit AIE) hace la semántica visible en el row.
    on_error           TEXT NOT NULL DEFAULT 'block_downstream'
                         CHECK (on_error IN ('block_downstream','continue')),
    -- `run_id` en la PK (audit AIE): aunque los task_id son UUID únicos por run (un par de tareas no
    -- colisiona entre runs), incluir run_id en la PK lo hace explícito y robusto, y alinea el índice
    -- de readiness por run. PK compuesta = arista única por (run, dependiente, dep).
    PRIMARY KEY (run_id, task_id, depends_on_task_id),
    FOREIGN KEY (run_id) REFERENCES pipeline_runs(id) ON DELETE CASCADE,
    FOREIGN KEY (task_id) REFERENCES orchestration_tasks(id) ON DELETE CASCADE,
    FOREIGN KEY (depends_on_task_id) REFERENCES orchestration_tasks(id) ON DELETE CASCADE
);

-- La PK (run_id, task_id, depends_on_task_id) ya cubre los filtros con prefijo run_id/task_id
-- (incluido el LEFT JOIN por task_id de `list_tasks`), así que NO se agrega índice por task_id
-- (sería redundante, audit AIE). El índice de readiness INVERSO es por la dep: un nodo se
-- desbloquea cuando TODAS las aristas que lo nombran como task_id tienen su depends_on_task_id
-- en `done`, y `on_task_done(dep)` busca las aristas DONDE depends_on_task_id = dep.
CREATE INDEX IF NOT EXISTS idx_pipeline_edges_dep ON pipeline_edges(depends_on_task_id);

-- ALTER aditivo sobre orchestration_tasks: pipeline_run_id (NULL = tarea single-task / batch
-- normal, comportamiento idéntico a hoy), dag_blocked (0 = lanzable; 1 = esperando deps — la
-- guarda de `claim_for_launch` lo rechaza), topo_index (orden de promoción dentro del run).
-- FK ON DELETE SET NULL (audit AIE): evita refs huérfanas a un run inexistente; si el run se
-- borra, la tarea revierte a single-task (pipeline_run_id NULL) en vez de quedar colgada. SQLite
-- admite ADD COLUMN con REFERENCES porque la columna acepta NULL y su default es NULL.
ALTER TABLE orchestration_tasks ADD COLUMN pipeline_run_id TEXT
    REFERENCES pipeline_runs(id) ON DELETE SET NULL;
-- CHECK (0,1) (audit AIE): la guarda de `claim_for_launch` (`AND dag_blocked=0`) asume binario;
-- un valor arbitrario rompería la lógica del gate. SQLite valida el CHECK en cada UPDATE/INSERT.
ALTER TABLE orchestration_tasks ADD COLUMN dag_blocked INTEGER NOT NULL DEFAULT 0
    CHECK (dag_blocked IN (0,1));
ALTER TABLE orchestration_tasks ADD COLUMN topo_index INTEGER;

CREATE INDEX IF NOT EXISTS idx_orch_tasks_pipeline ON orchestration_tasks(pipeline_run_id);
