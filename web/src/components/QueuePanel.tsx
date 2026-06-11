// 019 F2 T023 — superficie REAL que prueba el kit (US3): la "cola" de tareas de orquestación con
//   • FilterBar    (texto + faceta por estado)            → kit/FilterBar
//   • BulkActionBar (bulk-retry CONDICIONAL + progreso)   → kit/BulkActionBar
//   • DrillDown    (detalle por tarea)                    → kit/DrillDown
//   • InlineEdit   (renombrar el título in-situ)          → kit/InlineEdit
//
// GOBERNANZA (T022 + audit HIGH 3): TODA mutación pasa SIEMPRE por el `invoke` gobernado (gate
// universal → pending_approval → approvalBus → audit). El kit NO delega mutaciones a callbacks
// arbitrarios (un consumidor podría cablear un path que bypasee el gobierno). En concreto:
//   • bulk-cancel  → `invoke("orchestration_cancel", …)`         (gateado)
//   • bulk-retry   → `invoke("orchestration_prepare_task", …)`   (gateado; relaunch real, SSOT)
//   • inline-rename→ `invoke(renameCommandId, …)`                 (gateado; SÓLO si el Shell aporta
//                     un command id REAL del registry — sin él, rename queda deshabilitado, nunca
//                     se expone una mutación no-gobernada).
// El kit dispara intención; el backend (SSOT) muta y audita. Tokens V3, dark+light.
//
// ESTADO DE INTEGRACIÓN (audit ronda 2 H3): este panel es la DEMO de referencia de T023 — prueba el
// kit end-to-end con gobierno real y está LISTO PARA MONTARSE en el Shell. NO se monta acá: el
// reemplazo de OrchestrationBoard por QueuePanel es una MIGRACIÓN aparte (trabajo de integración
// posterior), fuera del alcance de este fix. Sirve como patrón canónico de cómo cablear el kit a
// `invoke` gobernado.
import { useCallback, useEffect, useMemo, useState } from "react";
import { invoke } from "../lib/invoke"; // gate universal + audit
import type { OrchTask } from "../types";
import { FilterBar } from "./kit/FilterBar";
import { BulkActionBar, type BulkAction } from "./kit/BulkActionBar";
import { DrillDown, type DrillRow } from "./kit/DrillDown";
import { InlineEdit } from "./kit/InlineEdit";
import { applyFilter, emptyFilter, facetCounts, type FilterState, type Facet } from "../lib/kit/filter";
import { toggle, toggleAll, pruneSelection, startBatch, advance, type BatchProgress } from "../lib/kit/selection";

/** Datos que `orchestration_prepare_task` (relaunch gobernado) devuelve para montar el pane. */
export interface PrepareTaskInfo {
  pane_id: string;
  worktree_path: string;
  mode: string;
  agent_profile_id: string | null;
  objective: string;
  session: string;
}

const STATE_COLOR: Record<OrchTask["state"], string> = {
  pending: "var(--ink-3, #635849)",
  running: "var(--accent)",
  awaiting_review: "var(--warn, #9a6011)",
  done: "var(--ok, #3a6b3f)",
  failed: "var(--clay, #b8543a)",
  canceled: "var(--ink-3, #635849)",
};
const STATE_LABEL: Record<OrchTask["state"], string> = {
  pending: "Pendiente", running: "Corriendo", awaiting_review: "Para revisar",
  done: "Hecha", failed: "Falló", canceled: "Cancelada",
};

export function QueuePanel({
  renameCommandId, onLaunched, onToast,
}: {
  /**
   * Audit HIGH 3 — command id REAL del registry para renombrar (debe ser una mutación gateada). Si
   * no se provee (no existe un comando de rename hoy), el InlineEdit queda DESHABILITADO: el kit
   * NUNCA expone una mutación que no pase por el gate.
   */
  renameCommandId?: string;
  /**
   * Hook OPCIONAL post-relaunch para que el Shell monte el pane del worktree devuelto por
   * `orchestration_prepare_task` (NO es la mutación — ésa la hace el `invoke` gobernado de adentro).
   */
  onLaunched?: (info: PrepareTaskInfo) => void;
  onToast: (kind: "success" | "error" | "info", msg: string) => void;
}) {
  const [tasks, setTasks] = useState<OrchTask[]>([]);
  const [filter, setFilter] = useState<FilterState>(emptyFilter());
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [progress, setProgress] = useState<BatchProgress | null>(null);
  const [busy, setBusy] = useState(false);

  const reload = useCallback(() => {
    invoke<OrchTask[]>("orchestration_list", { batchId: null })
      .then((t) => {
        setTasks(t);
        // no dejar fantasmas en la selección si una tarea desapareció (kit/selection).
        setSelected((sel) => pruneSelection(sel, t.map((x) => x.id)));
      })
      .catch(() => setTasks([]));
  }, []);
  useEffect(() => { reload(); const id = setInterval(reload, 3000); return () => clearInterval(id); }, [reload]);

  const filtered = useMemo(
    () => applyFilter(tasks, filter, {
      text: (t) => [t.title, t.objective, t.branch],
      facet: (t, f) => (f === "state" ? t.state : undefined),
    }),
    [tasks, filter],
  );

  const stateFacet: Facet = useMemo(() => {
    const counts = facetCounts(tasks, "state", (t, f) => (f === "state" ? t.state : undefined));
    return {
      id: "state", label: "Estado",
      options: (Object.keys(STATE_LABEL) as OrchTask["state"][])
        .filter((s) => counts[s])
        .map((s) => ({ value: s, label: STATE_LABEL[s], count: counts[s] })),
    };
  }, [tasks]);

  const byId = useMemo(() => new Map(tasks.map((t) => [t.id, t])), [tasks]);
  const stateOf = (id: string) => byId.get(id)?.state;

  // ejecuta una acción gobernada sobre cada id elegible, con progreso vivo. Falla de uno no corta.
  const runBulk = async (ids: string[], run: (id: string) => Promise<void>, verb: string) => {
    if (ids.length === 0) return;
    setBusy(true);
    let prog = startBatch(ids.length);
    setProgress(prog);
    let okCount = 0;
    // MED (audit) — try/finally: si onToast/reload lanzan, `busy` SIEMPRE se restaura (la barra no
    // queda bloqueada). El loop interno ya aísla el fallo de cada item (no corta el batch).
    try {
      for (const id of ids) {
        try { await run(id); okCount++; prog = advance(prog, true); }
        catch (e) { prog = advance(prog, false); onToast("error", `${verb} falló (${byId.get(id)?.title ?? id}): ${String(e)}`); }
        setProgress(prog);
      }
      onToast(okCount === ids.length ? "success" : "info", `${verb}: ${okCount}/${ids.length}.`);
      setSelected(new Set());
      reload();
    } finally {
      setBusy(false);
      setTimeout(() => setProgress(null), 1500);
    }
  };

  const actions: BulkAction[] = [
    {
      id: "retry", label: "Reintentar", variant: "accent",
      eligible: (id) => stateOf(id) === "failed", // bulk-retry CONDICIONAL: sólo las que fallaron
      // Audit HIGH 3 — relaunch REAL vía el `invoke` gobernado (gate universal + audit), igual que
      // bulk-cancel. NO se delega a un callback arbitrario. `orchestration_prepare_task` está gateado
      // (HIGH 4): produce pending_approval → aprobación → claim+worktree+audit en el backend (SSOT).
      onRun: (ids) => void runBulk(ids, async (id) => {
        const info = await invoke<PrepareTaskInfo>("orchestration_prepare_task", { taskId: id });
        onLaunched?.(info);
      }, "Reintentar"),
    },
    {
      id: "cancel", label: "Cancelar", variant: "clay",
      eligible: (id) => stateOf(id) === "pending" || stateOf(id) === "running",
      // MUTACIÓN gateada (audit H3): va SIEMPRE por el `invoke` gobernado (gate universal +
      // pending_approval + audit). El callback del kit jamás muta directo.
      onRun: (ids) => void runBulk(ids, (id) => invoke("orchestration_cancel", { taskId: id }), "Cancelar"),
    },
  ];

  const rows: DrillRow[] = filtered.map((t) => ({
    id: t.id,
    accent: STATE_COLOR[t.state],
    summary: (
      <div style={{ display: "flex", alignItems: "center", gap: 10 }}>
        <input
          type="checkbox"
          aria-label={`Seleccionar ${t.title}`}
          checked={selected.has(t.id)}
          onClick={(e) => e.stopPropagation()}
          onChange={() => setSelected((s) => toggle(s, t.id))}
        />
        <span style={{ flex: 1, minWidth: 0, fontWeight: 500, whiteSpace: "nowrap", overflow: "hidden", textOverflow: "ellipsis" }}>{t.title}</span>
        <span style={{ fontFamily: "var(--font-mono, monospace)", fontSize: 11, letterSpacing: ".06em", textTransform: "uppercase", color: STATE_COLOR[t.state] }}>{STATE_LABEL[t.state]}</span>
      </div>
    ),
    detail: () => (
      <div style={{ display: "flex", flexDirection: "column", gap: 8 }}>
        <div>
          <div style={{ fontFamily: "var(--font-mono, monospace)", fontSize: 10, letterSpacing: ".06em", textTransform: "uppercase", color: "var(--ink-3, #635849)", marginBottom: 2 }}>Título</div>
          {/* InlineEdit → renombrar in-situ SIEMPRE vía el `invoke` gobernado (gate + audit). Sólo se
              habilita si el Shell aporta un `renameCommandId` REAL del registry; sin él, queda
              deshabilitado (audit HIGH 3: el kit nunca expone una mutación no-gobernada). */}
          <InlineEdit
            value={t.title}
            ariaLabel="título de la tarea"
            disabled={!renameCommandId}
            onCommit={(next) => {
              if (!renameCommandId) return;
              // MUTACIÓN gateada (audit H3): el commit del kit persiste SIEMPRE vía el `invoke`
              // gobernado (gate universal + pending_approval + audit). Nunca un path directo.
              invoke(renameCommandId, { taskId: t.id, title: next })
                .then(reload)
                .catch((e) => onToast("error", `Renombrar falló: ${String(e)}`));
            }}
          />
        </div>
        <div style={{ fontFamily: "var(--font-mono, monospace)", fontSize: 12, color: "var(--ink-2, #5a5246)" }}>
          {t.branch}{t.objective ? ` · ${t.objective}` : ""}
        </div>
        {t.result_summary && <div style={{ fontSize: 12, color: "var(--ink-2, #5a5246)" }}>{t.result_summary}</div>}
      </div>
    ),
  }));

  const allIds = filtered.map((t) => t.id);
  return (
    <div style={{ display: "flex", flexDirection: "column", gap: 12, padding: 16, color: "var(--ink, #1c1814)", background: "var(--bg, #f3efe6)", height: "100%", overflowY: "auto" }}>
      <div style={{ fontFamily: "var(--font-display, serif)", fontSize: 20, fontWeight: 600 }}>Cola de tareas</div>
      <FilterBar state={filter} onChange={setFilter} facets={[stateFacet]} placeholder="filtrar por título / objetivo / branch…" />
      <div style={{ display: "flex", alignItems: "center", gap: 10 }}>
        <button
          type="button"
          onClick={() => setSelected((s) => toggleAll(s, allIds))}
          style={{ cursor: "pointer", fontSize: 12, background: "transparent", border: "1px solid var(--line, #d8d1bf)", borderRadius: "var(--radius, 3px)", padding: "4px 10px", color: "var(--ink-2, #5a5246)", fontFamily: "var(--font-sans, sans-serif)" }}
        >
          {allIds.length > 0 && allIds.every((id) => selected.has(id)) ? "Deseleccionar todo" : "Seleccionar todo"}
        </button>
        <span style={{ fontFamily: "var(--font-mono, monospace)", fontSize: 11, letterSpacing: ".06em", textTransform: "uppercase", color: "var(--ink-3, #635849)" }}>
          {filtered.length} de {tasks.length}
        </span>
      </div>
      <BulkActionBar selected={selected} actions={actions} progress={progress} busy={busy} onClear={() => setSelected(new Set())} />
      <DrillDown rows={rows} emptyLabel={tasks.length === 0 ? "no hay tareas todavía" : "ninguna tarea coincide con el filtro"} />
    </div>
  );
}
