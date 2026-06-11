// 022 US13 · L1 (refuerzo) — invariantes ESTRUCTURALES de la nav más allá de la cobertura.
// `node --experimental-strip-types`. Complementa navGroups.test.ts (que valida cobertura+i18n):
// acá enforzamos que la nav esté BIEN FORMADA como dato (IDs únicos, sin grupos vacíos, cada ítem
// con ícono + vista real). Esto ataca el corolario "data-driven verificado" del spec 022 (FR-021):
// si alguien agrega un ítem con una vista que NO existe en el union `View`, este test lo caza.
import { NAV_GROUPS, coveredViews } from "../navGroups.ts";
import { VIEWS } from "../router.ts";

let pass = 0, fail = 0;
function ok(cond: boolean, name: string) { if (cond) pass++; else { fail++; console.log(`FAIL ${name}`); } }

const viewSet = new Set<string>(VIEWS as readonly string[]);

// 1) IDs de grupo: únicos y no vacíos.
const groupIds = NAV_GROUPS.map((g) => g.id);
ok(new Set(groupIds).size === groupIds.length, "ids de grupo únicos");
ok(groupIds.every((id) => typeof id === "string" && id.length > 0), "ningún id de grupo vacío");

// 2) Ningún grupo sin ítems (un grupo vacío = caprichoso/cascarón).
ok(NAV_GROUPS.every((g) => g.items.length > 0), "ningún grupo de nav está vacío");

// 3) Cada ítem: tiene una vista REAL del union View, un ícono no vacío y un label fallback no vacío.
for (const g of NAV_GROUPS) {
  for (const it of g.items) {
    ok(viewSet.has(it.view), `ítem [${g.id}].${it.view}: la vista existe en el union View`);
    ok(typeof it.icon === "string" && it.icon.length > 0, `ítem [${g.id}].${it.view}: tiene ícono`);
    ok(typeof it.label === "string" && it.label.length > 0, `ítem [${g.id}].${it.view}: tiene label fallback`);
  }
}

// 4) Ninguna vista aparece en DOS grupos (una vista vive en exactamente un dominio).
const covered = coveredViews();
const dupes = covered.filter((v, i) => covered.indexOf(v) !== i);
ok(dupes.length === 0, `ninguna vista duplicada entre grupos (dupes: ${[...new Set(dupes)].join(",") || "ninguno"})`);

// 5) Cobertura total: toda vista del router está ruteada por la nav (sin huérfanas).
//    (navGroups.test.ts ya lo hace; lo re-afirmamos como invariante explícito del gate L1).
const coveredSet = new Set<string>(covered as readonly string[]);
const orphans = (VIEWS as readonly string[]).filter((v) => !coveredSet.has(v));
ok(orphans.length === 0, `0 vistas huérfanas (huérfanas: ${orphans.join(",") || "ninguna"})`);

console.log(`navWellFormed: ${pass} passed, ${fail} failed`);
process.exit(fail === 0 ? 0 : 1);
