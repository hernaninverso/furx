// 019 F2 T020 — lógica PURA de selección + elegibilidad de acción en lote (`BulkActionBar.tsx`
// es presentación). Destilada de BroadcastModal (Set<string> + aplicar a todos), OrchestrationBoard
// (descartar las no-elegidas), BestOfNCompare (descartar variantes). El "qué se puede accionar"
// (ej retry SÓLO sobre failed) es una REGLA, no UI → vive acá y se testea sin DOM.

export type SelectionSet = ReadonlySet<string>;

export function toggle(sel: SelectionSet, id: string): Set<string> {
  const next = new Set(sel);
  if (next.has(id)) next.delete(id);
  else next.add(id);
  return next;
}

/** Selecciona/deselecciona todos los ids dados (toggle "all": si ya están todos, los saca). */
export function toggleAll(sel: SelectionSet, allIds: readonly string[]): Set<string> {
  const allSelected = allIds.length > 0 && allIds.every((id) => sel.has(id));
  return allSelected ? new Set() : new Set(allIds);
}

/** Saca de la selección los ids que ya no existen en la lista (evita accionar sobre fantasmas). */
export function pruneSelection(sel: SelectionSet, presentIds: readonly string[]): Set<string> {
  const present = new Set(presentIds);
  return new Set([...sel].filter((id) => present.has(id)));
}

/**
 * Dada una selección y una `eligible(id) → boolean` (la condición de la acción, ej "está failed"),
 * parte en {actionable, blocked}. La BulkActionBar muestra "Retry (3)" y "2 no aplican" sin recalcular.
 * Pura.
 */
export function partitionEligible(
  sel: SelectionSet,
  eligible: (id: string) => boolean,
): { actionable: string[]; blocked: string[] } {
  const actionable: string[] = [];
  const blocked: string[] = [];
  for (const id of sel) (eligible(id) ? actionable : blocked).push(id);
  return { actionable, blocked };
}

/** Progreso de un lote en curso: para la barra "12/30 · 2 errores". Inmutable. */
export interface BatchProgress {
  total: number;
  done: number;
  errors: number;
}

export function startBatch(total: number): BatchProgress {
  return { total, done: 0, errors: 0 };
}

export function advance(p: BatchProgress, ok: boolean): BatchProgress {
  return { total: p.total, done: p.done + 1, errors: p.errors + (ok ? 0 : 1) };
}

export function isComplete(p: BatchProgress): boolean {
  return p.done >= p.total;
}

/** % 0..100 redondeado (para una barra). 0 total → 0. */
export function pct(p: BatchProgress): number {
  return p.total === 0 ? 0 : Math.round((p.done / p.total) * 100);
}
