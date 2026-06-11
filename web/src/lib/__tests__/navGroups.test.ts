// 015 T020 — tests de la nav agrupada: COBERTURA (ninguna vista huérfana) + el flag.
// `npm run test:nav` (Node type-strip).
import { NAV_GROUPS, coveredViews, navGroupLabelKey, navItemLabelKey } from "../navGroups.ts";
import { VIEWS } from "../router.ts";
import { es } from "../../locales/es.ts";
import { en } from "../../locales/en.ts";

let pass = 0, fail = 0;
function ok(cond: boolean, name: string) { if (cond) pass++; else { fail++; console.log(`FAIL ${name}`); } }

// 1) COBERTURA: cada vista del union View está en exactamente UN grupo (sin huérfanos como plugins).
const covered = coveredViews();
ok(covered.length === VIEWS.length, `cobertura count: ${covered.length} vs ${VIEWS.length}`);
for (const v of VIEWS) ok(covered.filter((c) => c === v).length === 1, `vista ${v} en exactamente 1 grupo`);
// 2) sin duplicados.
ok(new Set(covered).size === covered.length, "sin vistas duplicadas entre grupos");
// 3) 055 — espina lean (consenso del consejo): 4 dominios, 6 ítems exactos
//    (Sesiones, Cola, Memoria, Buscar, Actividad, Ajustes).
ok(NAV_GROUPS.length === 4, `4 dominios (got ${NAV_GROUPS.length})`);
const sidebarItems = NAV_GROUPS.flatMap((g) => g.items.map((i) => i.view));
ok(sidebarItems.length === 6, `6 ítems en la espina (got ${sidebarItems.length})`);
const SPINE = ["panes", "queue", "memory", "search", "activity", "settings"];
ok(SPINE.every((v) => sidebarItems.some((s) => s === v)), `espina = ${SPINE.join(",")}`);
// 4) 055/057 — las superficies demotadas NO están en el sidebar (siguen vivas como aliased/⌘K).
//    057 — `monitors` pasó a detalle detrás del Action Center "Actividad" (`activity`).
for (const v of ["extensions", "plugins", "tools", "github", "audit", "ssh", "vpn", "policy", "grafana", "monitors"]) {
  ok(!sidebarItems.some((s) => s === v), `${v} fuera del sidebar (aliased/⌘K)`);
}
// 5) ids de grupo únicos.
ok(new Set(NAV_GROUPS.map((g) => g.id)).size === NAV_GROUPS.length, "ids de grupo únicos");

// 6) i18n 1:1 (audit LOW 1): TODO grupo e ítem de NAV_GROUPS tiene su key en es.ts Y en.ts.
//    `navGroupLabelKey`/`navItemLabelKey` usan `as LocaleKey` (TS no garantiza existencia en runtime);
//    este test caza una vista/grupo nuevo sin su key del catálogo (fallaría silencioso, no a build-time).
const esKeys = new Set(Object.keys(es));
const enKeys = new Set(Object.keys(en));
for (const g of NAV_GROUPS) {
  const gk = navGroupLabelKey(g.id);
  ok(esKeys.has(gk), `key de grupo en es.ts: ${gk}`);
  ok(enKeys.has(gk), `key de grupo en en.ts: ${gk}`);
  for (const it of g.items) {
    const ik = navItemLabelKey(it.view);
    ok(esKeys.has(ik), `key de ítem en es.ts: ${ik}`);
    ok(enKeys.has(ik), `key de ítem en en.ts: ${ik}`);
  }
}

console.log(`navGroups: ${pass} passed, ${fail} failed`);
process.exit(fail === 0 ? 0 : 1);
