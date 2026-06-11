// 019 F2 T023 — segunda superficie que prueba el kit: el log de auditoría como VISTA (filtros +
// drill-down). Reusa kit/FilterBar (texto + faceta por kind) y kit/DrillDown (row→detalle). Es de
// SÓLO LECTURA (el audit es append-only, F0): no hay mutación → no toca el gate. Convive con el
// AuditDrawer existente sin cambiar su contrato. Tokens V3, dark+light. Sin "honest/honesto".
import { useMemo, useState } from "react";
import type { AuditEvent } from "../types";
import { fmtTime } from "../types";
import { FilterBar } from "./kit/FilterBar";
import { DrillDown, type DrillRow } from "./kit/DrillDown";
import { applyFilter, emptyFilter, facetCounts, type FilterState, type Facet } from "../lib/kit/filter";

function accentFor(kind: string): string {
  if (kind.startsWith("guardrail")) return "var(--warn, #9a6011)";
  if (kind.includes("error") || kind.includes("denied")) return "var(--err, #a8412c)";
  return "var(--accent)";
}

export function AuditPanel({ events }: { events: AuditEvent[] }) {
  const [filter, setFilter] = useState<FilterState>(emptyFilter());

  const filtered = useMemo(
    () => applyFilter(events, filter, {
      text: (e) => [e.kind, e.actor],
      facet: (e, f) => (f === "actor" ? e.actor : undefined),
    }),
    [events, filter],
  );

  const actorFacet: Facet = useMemo(() => {
    const counts = facetCounts(events, "actor", (e, f) => (f === "actor" ? e.actor : undefined));
    return {
      id: "actor", label: "Actor",
      options: Object.entries(counts)
        .sort((a, b) => b[1] - a[1])
        .slice(0, 8)
        .map(([value, count]) => ({ value, label: value, count })),
    };
  }, [events]);

  const rows: DrillRow[] = filtered.map((e) => ({
    id: e.id,
    accent: accentFor(e.kind),
    summary: (
      <div style={{ display: "flex", alignItems: "baseline", gap: 10 }}>
        <span style={{ fontFamily: "var(--font-mono, monospace)", fontSize: 11, color: "var(--ink-3, #635849)" }}>{fmtTime(e.at)}</span>
        <span style={{ flex: 1, minWidth: 0, fontWeight: 500, whiteSpace: "nowrap", overflow: "hidden", textOverflow: "ellipsis" }}>{e.kind}</span>
        <span style={{ fontFamily: "var(--font-mono, monospace)", fontSize: 11, color: "var(--ink-2, #5a5246)" }}>{e.actor}</span>
      </div>
    ),
    detail: () => (
      <div style={{ display: "flex", flexDirection: "column", gap: 4, fontFamily: "var(--font-mono, monospace)", fontSize: 12, color: "var(--ink-2, #5a5246)" }}>
        <div>id: {e.id}</div>
        <div>kind: {e.kind}</div>
        <div>actor: {e.actor}</div>
        <div>at: {e.at}</div>
      </div>
    ),
  }));

  return (
    <div style={{ display: "flex", flexDirection: "column", gap: 12, padding: 16, color: "var(--ink, #1c1814)", background: "var(--bg, #f3efe6)", height: "100%", overflowY: "auto" }}>
      <div style={{ fontFamily: "var(--font-display, serif)", fontSize: 20, fontWeight: 600 }}>Auditoría</div>
      <FilterBar state={filter} onChange={setFilter} facets={[actorFacet]} placeholder="filtrar por kind / actor…" />
      <span style={{ fontFamily: "var(--font-mono, monospace)", fontSize: 11, letterSpacing: ".06em", textTransform: "uppercase", color: "var(--ink-3, #635849)" }}>
        {filtered.length} de {events.length} evento(s)
      </span>
      <DrillDown rows={rows} emptyLabel={events.length === 0 ? "sin eventos" : "ningún evento coincide"} />
    </div>
  );
}
