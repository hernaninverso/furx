// 022 US13 · L1 (refuerzo) — invariante CROSS-MODULE: cada stat es una "puerta a una acción"
// que aterriza en una vista REALMENTE alcanzable por la nav, y el drill-down produce un NavState
// coherente con su filtro one-shot. `node --experimental-strip-types`.
//
// stats.test.ts ya valida la forma de cada stat aislada; ESTE test cruza stats ↔ navGroups ↔
// reducer de navegación para garantizar el corolario del spec 022 ("cada elemento visible es
// accionable o navegable"): un stat NO puede apuntar a una vista huérfana, y al accionarlo el
// estado de navegación resultante debe aplicar (o limpiar) el filtro correcto.
import { buildSidebarStats, statFilterToViewFilter, nextNavState } from "../stats.ts";
import { coveredViews } from "../navGroups.ts";

let pass = 0, fail = 0;
function ok(cond: boolean, name: string) { if (cond) pass++; else { fail++; console.log(`FAIL ${name}`); } }

const navReachable = new Set<string>(coveredViews() as readonly string[]);
const stats = buildSidebarStats({ openIncidents: 7, panes: 2, monitorsUp: 1, monitorsTotal: 4 });

// 1) Cobertura cross-module: el destino de TODO stat está en la nav (no es una vista huérfana).
for (const s of stats) {
  ok(navReachable.has(s.destView), `stat ${s.id} → destino ${s.destView} alcanzable en la nav`);
}

// 2) Al accionar el stat (drill-down): el NavState resultante apunta a la vista del stat y aplica
//    EXACTAMENTE el ViewFilter de su drill-down (one-shot). Esto fija el invariante de "ir a la causa".
for (const s of stats) {
  const vf = statFilterToViewFilter(s);
  const nav = nextNavState(s.destView, vf);
  ok(nav.view === s.destView, `drill-down de ${s.id} navega a ${s.destView}`);
  ok(JSON.stringify(nav.filter) === JSON.stringify(vf), `drill-down de ${s.id} aplica su filtro one-shot`);
}

// 3) Drill-down específicos: incidentes → abiertos, monitors → caídos, panes → sin filtro.
const byId = new Map(stats.map((s) => [s.id, s]));
ok(JSON.stringify(statFilterToViewFilter(byId.get("incidents")!)) === JSON.stringify({ view: "incidents", status: "open" }), "incidents drill → open");
ok(JSON.stringify(statFilterToViewFilter(byId.get("monitors")!)) === JSON.stringify({ view: "monitors", status: "down" }), "monitors drill → down");
ok(statFilterToViewFilter(byId.get("panes")!) === null, "panes drill → sin filtro");

// 4) One-shot: re-entrar a la misma vista por una nav NORMAL (sin filtro) limpia el drill-down.
//    (re-entrar a Incidents por el sidebar NO debe heredar el filtro del stat).
const reentry = nextNavState("incidents");
ok(reentry.view === "incidents" && reentry.filter === null, "nav normal a incidents limpia el filtro (one-shot)");

// 5) Edge: con valores 0, el destino sigue siendo navegable (no se vuelve inerte).
const zero = buildSidebarStats({ openIncidents: 0, panes: 0, monitorsUp: 0, monitorsTotal: 0 });
for (const s of zero) ok(navReachable.has(s.destView), `stat ${s.id} en 0 sigue navegable`);

console.log(`statsDrilldown: ${pass} passed, ${fail} failed`);
process.exit(fail === 0 ? 0 : 1);
