// 022 P0c · US5/FR-008 — guard de regresión: los literales de chrome MIGRADOS no deben volver a
// aparecer como texto suelto en los archivos de la chrome (TopBar / CardsRail / Shell zona stats+
// shortcuts+nav). NO es un scanner genérico de JSX (sería ruidoso por glifos/no-prosa): es un check
// acotado y determinista sobre los literales concretos que reemplazamos por `t()`. Cubre FR-008 sin
// falsos positivos. `node --experimental-strip-types`.
import { readFileSync } from "node:fs";

let pass = 0, fail = 0;
function ok(cond: boolean, name: string) { if (cond) pass++; else { fail++; console.log(`FAIL ${name}`); } }

function read(rel: string): string {
  // tests corren desde la raíz del repo (scripts/test-all.mjs) → ruta web/src/...
  return readFileSync(new URL(`../../${rel}`, import.meta.url), "utf8");
}

const topbar = read("components/TopBar.tsx");
const rail = read("components/CardsRail.tsx");
const shell = read("Shell.tsx");

// Literales que YA NO deben aparecer crudos (fueron movidos al catálogo i18n).
// Se busca el string ENTRE COMILLAS para no chocar con comentarios/keys del catálogo.
function noRawLiteral(src: string, literals: string[], where: string) {
  for (const lit of literals) {
    const present = src.includes(`"${lit}"`) || src.includes(`'${lit}'`) || src.includes(`>${lit}<`) || src.includes(`\`${lit}`);
    ok(!present, `${where}: sin literal crudo "${lit}"`);
  }
}

// TopBar — labels/titles/aria migrados.
noRawLiteral(topbar, [
  "no usage data", "Daily standup", "Open standup", "Auto-PR description",
  "Open PR description", "Smart paste — analizar clipboard", "Audit drawer (live)",
  "Toggle audit drawer", "Switch to light", "Switch to dark", "Toggle theme",
  "Centro de ayuda", "Abrir ayuda",
], "TopBar");
// Los pills ya no llevan el texto pegado al glifo como literal.
ok(!topbar.includes("✱ standup") && !topbar.includes("⇪ pr") && !topbar.includes("≡ audit"), "TopBar: pills sin texto-literal pegado al glifo");

// CardsRail — header/aria migrados.
noRawLiteral(rail, [
  "All quiet.", "Collapse incidents panel", "Collapse",
  "Expand incidents panel", "Incidents · click to expand",
], "CardsRail");
ok(!rail.includes("Incidents · {openCount} open") && !rail.includes("Incidents · ${openCount}"), "CardsRail: header sin literal");

// Shell — zona stats footer + shortcuts + nav toggle + dev (los literales que migramos a t()).
noRawLiteral(shell, [
  "Estado del workspace", "Actualizado", "Shortcuts:", "Ver todos los shortcuts",
  "ver todos los shortcuts", "Seed demo cards (dev)", "All quiet.",
], "Shell");
// El toggle de nav ya no arma "Nav: agrupada/plana" como literal.
ok(!shell.includes('"agrupada"') && !shell.includes('"plana"') && !shell.includes("Nav: {navGrouped"), "Shell: nav toggle sin literal");

console.log(`chromeI18n: ${pass} passed, ${fail} failed`);
process.exit(fail === 0 ? 0 : 1);
