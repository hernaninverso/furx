// 022 P0c · US5/FR-009 — el catálogo base (es.ts) MUST seguir sentence-case (enforced, no manual).
// `node --experimental-strip-types`. Recorre TODAS las keys del catálogo fuente y falla con el
// detalle de las que violan la convención (Title-Case en medio o minúscula al arranque de prosa).
import { es } from "../../locales/es.ts";
import { checkSentenceCase, PROPER_NOUNS, KNOWN_ACRONYMS } from "../sentenceCase.ts";

let pass = 0, fail = 0;
function ok(cond: boolean, name: string) { if (cond) pass++; else { fail++; console.log(`FAIL ${name}`); } }

// ── 1) Cada valor del catálogo SOURCE respeta sentence-case ──────────────────────────────────────
// FRAGMENT_KEYS (brand wave 4, 2026-06-09): keys cuyo valor es un FRAGMENTO de oración que se
// compone dentro de JSX (sufijos tras un <code>/<kbd>/placeholder, conectores, tiempos relativos).
// Para ellas NO se exige mayúscula inicial ("first-not-upper"); el resto de la convención
// (mid-title-case) aplica igual. NO agregar acá prosa completa — solo fragmentos reales.
const FRAGMENT_KEYS = new Set([
  "wizard.health.ok", "wizard.health.bad",
  "connect.err.noDetail", "connect.local.notDetected", "connect.local.noneOr",
  "connect.local.nonePost",
  "accounts.namePlaceholder", "accounts.rel.now", "accounts.rel.m", "accounts.rel.h",
  "empty.hintClaudeUnset", "empty.hintZsh", "empty.hintWizard", "empty.hintProviders",
  "empty.scNew", "empty.scActions", "empty.scAll", "empty.tmuxMissing", "empty.tmuxReopen",
  "chrome.panes.grid2x2", "chrome.panes.autoLayout",
  "council.running", "council.convening", "council.templateOptional", "council.voiceModelDefault",
]);
const violations: { key: string; value: string; offenders: string }[] = [];
for (const [key, value] of Object.entries(es)) {
  const r = checkSentenceCase(String(value));
  const offenders = FRAGMENT_KEYS.has(key)
    ? r.offenders.filter((o) => o.reason !== "first-not-upper")
    : r.offenders;
  if (offenders.length > 0) {
    violations.push({ key, value: String(value), offenders: offenders.map((o) => `${o.token}[${o.reason}]`).join(", ") });
  }
}
if (violations.length) {
  console.log(`\n  ${violations.length} violación(es) de sentence-case en el catálogo base:`);
  for (const v of violations) console.log(`    ${v.key} = ${JSON.stringify(v.value)} → ${v.offenders}`);
}
ok(violations.length === 0, "0 violaciones de sentence-case en es.ts");

// ── 2) Cobertura del catálogo (que el recorrido no esté vacío por un import roto) ────────────────
ok(Object.keys(es).length > 0, "el catálogo base tiene al menos una key");

// ── 3) La regla DETECTA Title-Case real (no es un no-op) ─────────────────────────────────────────
ok(!checkSentenceCase("Incidentes Abiertos").ok, "detecta Title-Case ('Incidentes Abiertos')");
ok(checkSentenceCase("Incidentes abiertos").ok, "acepta sentence-case ('Incidentes abiertos')");

// ── 4) Allowlist de nombres propios / siglas ─────────────────────────────────────────────────────
ok(checkSentenceCase("Novedades de Furx").ok, "permite nombre propio en allowlist (Furx)");
ok(checkSentenceCase("Abrir descripción de PR").ok, "permite sigla en allowlist (PR)");
ok(checkSentenceCase("Cambiar a oscuro").ok, "sentence-case simple ok");
ok(PROPER_NOUNS.includes("AIE"), "allowlist incluye AIE");

// ── 5) Arranque dinámico (placeholder / dígito) no exige mayúscula ───────────────────────────────
ok(checkSentenceCase("{count} entradas").ok, "valor que arranca con placeholder no exige mayúscula");
ok(checkSentenceCase("24h").ok, "valor que arranca con dígito no exige mayúscula");

// ── 6) Nueva oración tras '.'/':' puede ir en mayúscula ──────────────────────────────────────────
ok(checkSentenceCase("Idioma de la interfaz. El texto cae al original.").ok, "mayúscula tras punto ok");
ok(checkSentenceCase("Métricas agregadas. Por defecto: deshabilitado.").ok, "mayúscula tras punto ok 2");

// ── 7) Allowlist de siglas ESTRICTA (audit MED 3): all-caps arbitrario NO listado → falla ────────
ok(!checkSentenceCase("Incidentes ABIERTOS").ok, "all-caps arbitrario en medio falla ('Incidentes ABIERTOS')");
ok(checkSentenceCase("Estado de AIE").ok, "sigla allowlistada (AIE) pasa");
ok(checkSentenceCase("Configurar SSH").ok, "sigla allowlistada (SSH) pasa");
ok(checkSentenceCase("Exportar a JSON").ok, "sigla conocida (JSON) pasa");
ok(!checkSentenceCase("Enviar a XYZ").ok, "sigla all-caps NO listada (XYZ) falla");
ok(KNOWN_ACRONYMS.includes("JSON"), "KNOWN_ACRONYMS incluye JSON");

console.log(`sentenceCase: ${pass} passed, ${fail} failed`);
process.exit(fail === 0 ? 0 : 1);
