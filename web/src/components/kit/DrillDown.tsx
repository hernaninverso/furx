// 019 F2 T020 — DrillDown: master → detalle expandible. Destilado del `<details>` de DataViewer,
// del log-history expandible de OrchestrationBoard y del row→detalle de AuditDrawer. PRESENTACIÓN
// pura: el detalle es lazy (sólo se pide `renderDetail` cuando se abre). Tokens V3, dark+light.
import { useState, type ReactNode } from "react";
import { kitLbl } from "./styles";

export interface DrillRow {
  id: string;
  /** resumen siempre visible (la "master" line). */
  summary: ReactNode;
  /** detalle perezoso: sólo se llama cuando la fila está abierta. */
  detail: () => ReactNode;
  /** acento del borde izquierdo (token V3). */
  accent?: string;
}

export function DrillDown({ rows, emptyLabel = "sin elementos" }: { rows: DrillRow[]; emptyLabel?: string }) {
  const [open, setOpen] = useState<Set<string>>(new Set());
  const toggle = (id: string) =>
    setOpen((s) => {
      const n = new Set(s);
      n.has(id) ? n.delete(id) : n.add(id);
      return n;
    });

  if (rows.length === 0) {
    return <div style={{ ...kitLbl, padding: 12, textAlign: "center" }}>{emptyLabel}</div>;
  }
  return (
    <div style={{ display: "flex", flexDirection: "column", gap: 4 }}>
      {rows.map((r) => {
        const isOpen = open.has(r.id);
        return (
          <div
            key={r.id}
            style={{
              border: "1px solid var(--line, #d8d1bf)",
              borderLeft: `3px solid ${r.accent ?? "var(--line-2, #c4bba4)"}`,
              borderRadius: "var(--radius, 3px)", overflow: "hidden",
            }}
          >
            <button
              type="button"
              aria-expanded={isOpen}
              onClick={() => toggle(r.id)}
              style={{
                width: "100%", textAlign: "left", cursor: "pointer",
                display: "flex", alignItems: "center", gap: 8, padding: "8px 10px",
                background: isOpen ? "var(--bg-2, #ece7da)" : "transparent",
                border: "none", color: "var(--ink, #1c1814)",
                fontFamily: "var(--font-sans, sans-serif)", fontSize: 14,
              }}
            >
              <span aria-hidden style={{ fontFamily: "var(--font-mono, monospace)", color: "var(--ink-3, #635849)" }}>{isOpen ? "▾" : "▸"}</span>
              <span style={{ flex: 1, minWidth: 0 }}>{r.summary}</span>
            </button>
            {isOpen && (
              <div style={{ padding: "8px 12px", borderTop: "1px solid var(--line, #d8d1bf)" }}>
                {r.detail()}
              </div>
            )}
          </div>
        );
      })}
    </div>
  );
}
