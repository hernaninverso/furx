// 022 P0b · REFORMA 3 — tests de la lógica pura de stats accionables. `node --experimental-strip-types`.
// Cubre: cada stat tiene un destView real, el mapeo stat→ViewFilter, NO hay "Schema v3",
// los valores derivan de los inputs, y la etiqueta de freshness.
import { buildSidebarStats, statFilterToViewFilter, freshnessLabel } from "../stats.ts";
import { VIEWS } from "../router.ts";

let pass = 0, fail = 0;
function ok(cond: boolean, name: string) { if (cond) pass++; else { fail++; console.log(`FAIL ${name}`); } }

const stats = buildSidebarStats({ openIncidents: 29, panes: 4, monitorsUp: 2, monitorsTotal: 3 });

// 1) Cada stat tiene un destView que existe DE VERDAD en el union View (FR-005/FR-021).
const viewSet = new Set<string>(VIEWS as readonly string[]);
ok(stats.length === 3, "3 stats (incidents, panes, monitors) — sin Schema");
ok(stats.every((s) => viewSet.has(s.destView)), "cada stat navega a una View real");
ok(stats.every((s) => s.ariaLabel.length > 0), "cada stat tiene aria-label");

// 2) NO existe ningún stat "Schema" / literal muerto.
ok(!stats.some((s) => /schema/i.test(s.label) || /schema/i.test(s.id)), "0 stat 'Schema' (eliminado)");

// 3) Valores derivados de los inputs (no hardcode).
const byId = new Map(stats.map((s) => [s.id, s]));
ok(byId.get("incidents")!.value === "29", "incidentes = openIncidents");
ok(byId.get("panes")!.value === "4", "paneles = panes");
ok(byId.get("monitors")!.value === "2/3 arriba", "monitors = up/total (localizado, fallback ES)");

// 4) Destinos + filtros de drill-down correctos.
ok(byId.get("incidents")!.destView === "incidents", "incidentes → vista incidents");
ok(byId.get("monitors")!.destView === "monitors", "monitors → vista monitors");
ok(byId.get("panes")!.destView === "panes", "paneles → vista panes");
ok(byId.get("incidents")!.filter.kind === "incidents", "incidentes filtro=incidents");
ok(byId.get("monitors")!.filter.kind === "monitors", "monitors filtro=monitors");
ok(byId.get("panes")!.filter.kind === "none", "paneles sin filtro");

// 5) statFilterToViewFilter mapea bien.
ok(JSON.stringify(statFilterToViewFilter(byId.get("incidents")!)) === JSON.stringify({ view: "incidents", status: "open" }), "incidents → ViewFilter abiertos");
ok(JSON.stringify(statFilterToViewFilter(byId.get("monitors")!)) === JSON.stringify({ view: "monitors", status: "down" }), "monitors → ViewFilter down");
ok(statFilterToViewFilter(byId.get("panes")!) === null, "panes → sin ViewFilter");

// 6) Edge: valor 0 sigue navegando (no se vuelve inerte).
const zero = buildSidebarStats({ openIncidents: 0, panes: 0, monitorsUp: 0, monitorsTotal: 0 });
ok(zero.every((s) => viewSet.has(s.destView)), "stats en 0 siguen teniendo destino");
ok(zero.find((s) => s.id === "monitors")!.value === "0/0 arriba", "monitors 0/0 (localizado)");

// 7) Freshness: relativo, fail-soft. Labels vía catálogo (fallback ES con el translator default).
const now = 10_000_000;
ok(freshnessLabel(now - 3000, now) === "Hace 3s", "Hace 3s");
ok(freshnessLabel(now - 120_000, now) === "Hace 2m", "Hace 2m");
ok(freshnessLabel(now - 7_200_000, now) === "Hace 2h", "Hace 2h");
ok(freshnessLabel(null, now) === "", "null → vacío");
ok(freshnessLabel(now + 5000, now) === "", "futuro → vacío");
ok(freshnessLabel(now - 500, now) === "Recién", "<1s → Recién");
// 7b) Translator inyectado (locale en) → labels en inglés (audit MED 1, paridad es↔en).
const tEn = (key: string, p?: Record<string, string | number>) =>
  key === "chrome.stats.freshSecs" ? `${p?.n}s ago`
    : key === "chrome.stats.freshNow" ? "Just now"
      : String(key);
ok(freshnessLabel(now - 3000, now, tEn as never) === "3s ago", "freshness respeta translator (en)");
ok(freshnessLabel(now - 500, now, tEn as never) === "Just now", "freshNow respeta translator (en)");

console.log(`stats: ${pass} passed, ${fail} failed`);
process.exit(fail === 0 ? 0 : 1);
