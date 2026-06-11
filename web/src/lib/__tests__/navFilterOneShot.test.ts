// 022 P0b (audit 3-frontera MED) — el filtro de drill-down es ONE-SHOT. Reproduce el escenario
// del bug: entrar a Incidents POR UN STAT (filtro abiertos) → ir a otra vista → re-entrar a
// Incidents por nav NORMAL (sidebar/palette) → NO debe quedar filtrado. El reducer puro
// `nextNavState` modela el estado (view+filter) que el Shell pinta. `node --experimental-strip-types`.
import {
  nextNavState,
  buildSidebarStats,
  statFilterToViewFilter,
  type ViewFilter,
} from "../stats.ts";

let pass = 0, fail = 0;
function ok(cond: boolean, name: string) { if (cond) pass++; else { fail++; console.log(`FAIL ${name}`); } }
function eq(a: unknown, b: unknown): boolean { return JSON.stringify(a) === JSON.stringify(b); }

const stats = buildSidebarStats({ openIncidents: 3, panes: 2, monitorsUp: 1, monitorsTotal: 3 });
const incStat = stats.find((s) => s.id === "incidents")!;
const monStat = stats.find((s) => s.id === "monitors")!;

// goToStat: setea vista + filtro ATÓMICAMENTE (one-shot encendido).
const afterStat = nextNavState(incStat.destView, statFilterToViewFilter(incStat));
ok(afterStat.view === "incidents", "stat → vista incidents");
ok(eq(afterStat.filter, { view: "incidents", status: "open" }), "stat → filtro abiertos aplicado");

// nav NORMAL a otra vista (sidebar/palette) — sin filtro: se limpia.
const afterPanes = nextNavState("panes");
ok(afterPanes.view === "panes", "nav normal → panes");
ok(afterPanes.filter === null, "nav normal limpia el filtro");

// re-entrar a Incidents por nav NORMAL (sidebar) → NO filtrado (el bug original lo dejaba pegado).
const reIncidents = nextNavState("incidents");
ok(reIncidents.view === "incidents", "re-entrada por sidebar → incidents");
ok(reIncidents.filter === null, "re-entrada por nav normal NO filtra (one-shot)");

// el mismo invariante para monitors.
const afterMonStat = nextNavState(monStat.destView, statFilterToViewFilter(monStat));
ok(eq(afterMonStat.filter, { view: "monitors", status: "down" }), "stat monitors → filtro down");
const reMonitors = nextNavState("monitors");
ok(reMonitors.filter === null, "re-entrada monitors por nav normal NO filtra");

// nav normal NUNCA arrastra un filtro previo (default null) — exhaustivo sobre destinos.
const destinations: ("incidents" | "monitors" | "panes" | "latency")[] = [
  "incidents", "monitors", "panes", "latency",
];
for (const d of destinations) {
  const st = nextNavState(d);
  ok(st.filter === null, `nav normal a ${d} → filtro null`);
}

// goToStat sigue siendo el ÚNICO que produce filtro no-null.
const filterProducing: ViewFilter = nextNavState(incStat.destView, statFilterToViewFilter(incStat)).filter;
ok(filterProducing !== null, "goToStat sí produce filtro (no inerte)");

console.log(`navFilterOneShot: ${pass} passed, ${fail} failed`);
if (fail > 0) process.exit(1);
