// 019 F2 T020 — BulkActionBar: barra de acción en lote sobre una selección, con progreso vivo.
// PRESENTACIÓN pura; la elegibilidad (qué ids aplican) y el progreso viven en `lib/kit/selection.ts`.
// CADA acción del consumidor debe ejecutarse vía el `invoke` gobernado (gate universal + audit) —
// la barra no ejecuta nada por su cuenta; sólo dispara el callback `onRun` que el consumidor cablea.
// Tokens V3, dark+light. Sin "honest/honesto".
import type { ReactNode } from "react";
import type { BatchProgress } from "../../lib/kit/selection";
import { partitionEligible, pct } from "../../lib/kit/selection";
import { kitBtn, kitLbl } from "./styles";

export interface BulkAction {
  id: string;
  label: string;
  variant?: "accent" | "clay";
  /** qué ids de la selección son elegibles para ESTA acción (ej retry → sólo failed). */
  eligible: (id: string) => boolean;
  /**
   * Dispara la acción. El consumidor recibe SÓLO los ids elegibles.
   * INVARIANTE DE GOBIERNO (audit ronda 2 H3): si esta acción MUTA estado, el consumidor DEBE
   * ejecutarla vía el `invoke` gobernado (web/src/lib/invoke.ts → pending_approval → approvalBus →
   * audit). El kit sólo dispara intención; NUNCA cablear `onRun` a un fetch/invoke directo que
   * bypasee el gate. Ver QueuePanel.tsx como referencia.
   */
  onRun: (ids: string[]) => void;
}

export function BulkActionBar({
  selected, actions, progress, busy, onClear, extra,
}: {
  selected: ReadonlySet<string>;
  actions: BulkAction[];
  progress?: BatchProgress | null;
  busy?: boolean;
  onClear?: () => void;
  extra?: ReactNode;
}) {
  if (selected.size === 0 && !progress) return null;
  return (
    <div
      role="toolbar"
      aria-label="Acciones en lote"
      style={{
        display: "flex", alignItems: "center", gap: 10, flexWrap: "wrap",
        padding: "8px 12px", borderRadius: "var(--radius-lg, 5px)",
        background: "var(--bg-2, #ece7da)", border: "1px solid var(--line, #d8d1bf)",
      }}
    >
      <span style={kitLbl}>{selected.size} seleccionado(s)</span>
      {actions.map((a) => {
        const { actionable, blocked } = partitionEligible(selected, a.eligible);
        return (
          <button
            key={a.id}
            type="button"
            style={kitBtn(a.variant)}
            disabled={busy || actionable.length === 0}
            title={blocked.length ? `${blocked.length} no aplica(n) a esta acción` : undefined}
            onClick={() => a.onRun(actionable)}
          >
            {a.label}{actionable.length ? ` (${actionable.length})` : ""}
          </button>
        );
      })}
      {extra}
      {onClear && (
        <button type="button" style={kitBtn()} disabled={busy} onClick={onClear}>
          Limpiar selección
        </button>
      )}
      {progress && (
        <span style={{ display: "flex", alignItems: "center", gap: 8, marginLeft: "auto" }}>
          <span style={kitLbl}>
            {progress.done}/{progress.total}
            {progress.errors > 0 ? ` · ${progress.errors} error(es)` : ""}
          </span>
          <span aria-hidden style={{ width: 120, height: 6, borderRadius: 999, background: "var(--bg-3, #e1dccb)", overflow: "hidden" }}>
            <span style={{ display: "block", height: "100%", width: `${pct(progress)}%`, background: progress.errors > 0 ? "var(--warn, #9a6011)" : "var(--accent)", transition: "width var(--dur, 240ms) var(--ease, ease)" }} />
          </span>
        </span>
      )}
    </div>
  );
}
