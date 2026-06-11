// 022 US7 / FR-012 — entrada a Cuentas/Perfiles desde Ajustes. Verifica el CONTRATO i18n de las
// claves nuevas (sección + dos acciones) sin renderizar React: que existan en ambos locales, que
// resuelvan a copy real y que respeten sentence-case (US5). El wiring de apertura (ConnectScreen
// initialTab + evento `furx:open-agents`) se ejercita en el smoke E2E de GUI (se corre manualmente);
// acá fijamos lo testeable como lógica pura. `node --experimental-strip-types`.
import { es } from "../../locales/es.ts";
import { en } from "../../locales/en.ts";
import { translate } from "../i18n.ts";
import { checkSentenceCase } from "../sentenceCase.ts";

let pass = 0, fail = 0;
function ok(cond: boolean, name: string) { if (cond) pass++; else { fail++; console.log(`FAIL ${name}`); } }

// Las 6 claves que materializan la entrada en Ajustes (sección + hint + 2 acciones con su hint).
const KEYS = [
  "settings.accounts.section",
  "settings.accounts.hint",
  "settings.accounts.manage",
  "settings.accounts.manageHint",
  "settings.accounts.gallery",
  "settings.accounts.galleryHint",
] as const;

// 1) Presentes en AMBOS locales (paridad — el guard global ya lo cubre, acá lo fijamos explícito).
for (const k of KEYS) {
  ok(k in es, `es.ts tiene "${k}"`);
  ok(k in en, `en.ts tiene "${k}"`);
}

// 2) Resuelven a copy NO vacío en ambos idiomas (nunca key cruda).
for (const k of KEYS) {
  const sv = translate("es", k);
  const ev = translate("en", k);
  ok(sv.length > 0 && sv !== k, `es resuelve "${k}" a copy real`);
  ok(ev.length > 0 && ev !== k, `en resuelve "${k}" a copy real`);
}

// 3) El source respeta sentence-case (US5/FR-009) para las claves nuevas.
for (const k of KEYS) {
  ok(checkSentenceCase(translate("es", k)).ok, `"${k}" es sentence-case en es`);
}

// 4) Las dos acciones apuntan a conceptos distintos (cuentas vs galería de agentes).
ok(translate("es", "settings.accounts.manage") !== translate("es", "settings.accounts.gallery"),
  "las dos acciones tienen labels distintos");

// 5) F-III — cero "honesto"/"honest" en el copy nuevo.
for (const k of KEYS) {
  const both = (translate("es", k) + " " + translate("en", k)).toLowerCase();
  ok(!both.includes("honest"), `"${k}" sin la palabra honesto/honest`);
}

console.log(`accountsSettings: ${pass} passed, ${fail} failed`);
process.exit(fail === 0 ? 0 : 1);
