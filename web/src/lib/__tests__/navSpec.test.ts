// 017 T002 — cobertura SSOT del NavSpec móvil: todo id de MOBILE_NAV_SUBSET existe
// en NAV_GROUPS (falla si hay id huérfano), y buildNavSpec() no inventa items.
// Corre con `node --experimental-strip-types` vía `scripts/test-all.mjs`.
import {
  NAV_GROUPS,
  MOBILE_NAV_SUBSET,
  MOBILE_NAV_SPEC_VERSION,
  buildNavSpec,
} from "../navGroups.ts";

let pass = 0,
  fail = 0;
function ok(cond: boolean, name: string) {
  if (cond) pass++;
  else {
    fail++;
    console.log(`FAIL ${name}`);
  }
}

const groupIds = new Set(NAV_GROUPS.map((g) => g.id));

// 1) COBERTURA SSOT: cada id del subset móvil existe en NAV_GROUPS (sin huérfanos).
for (const id of MOBILE_NAV_SUBSET) {
  ok(groupIds.has(id), `subset id "${id}" existe en NAV_GROUPS`);
}

// 2) subset sin duplicados.
ok(
  new Set(MOBILE_NAV_SUBSET).size === MOBILE_NAV_SUBSET.length,
  "MOBILE_NAV_SUBSET sin duplicados",
);

// 3) buildNavSpec materializa exactamente los dominios del subset, en orden.
const spec = buildNavSpec();
ok(spec.version === MOBILE_NAV_SPEC_VERSION, "spec.version == MOBILE_NAV_SPEC_VERSION");
ok(
  spec.domains.length === MOBILE_NAV_SUBSET.length,
  `domains count ${spec.domains.length} == subset ${MOBILE_NAV_SUBSET.length}`,
);
spec.domains.forEach((d, i) => {
  ok(d.domainId === MOBILE_NAV_SUBSET[i], `domain[${i}] orden preservado (${d.domainId})`);
});

// 4) buildNavSpec NO inventa items: cada item del spec sale tal cual de NAV_GROUPS.
for (const d of spec.domains) {
  const src = NAV_GROUPS.find((g) => g.id === d.domainId)!;
  ok(d.label === src.label, `domain ${d.domainId} label SSOT`);
  ok(d.items.length === src.items.length, `domain ${d.domainId} item count SSOT`);
  for (const it of d.items) {
    const m = src.items.find((s) => s.view === it.view);
    ok(!!m, `item ${it.view} existe en NAV_GROUPS[${d.domainId}]`);
    ok(!!m && m.label === it.label && m.icon === it.icon, `item ${it.view} label+icon SSOT`);
  }
}

// 5) los dominios EXCLUIDOS del móvil no se filtran (infra/extensions/system fuera).
const included = new Set(spec.domains.map((d) => d.domainId));
ok(!included.has("infra"), "infra EXCLUIDO del subset móvil");
ok(!included.has("extensions"), "extensions EXCLUIDO del subset móvil");
ok(!included.has("system"), "system EXCLUIDO del subset móvil");

console.log(`navSpec: ${pass} passed, ${fail} failed`);
process.exit(fail === 0 ? 0 : 1);
