// 020 meta-orchestrator US2/US3 — tests de la lógica pura de sugerencias advisory.
// Estilo node:assert (sin DOM, sin vitest). `npm test` lo descubre vía scripts/test-all.mjs.
import assert from "node:assert/strict";
import {
  variantLabel,
  parseRankingSuggestion,
  agentCategoryDisplay,
  variantsContentKey,
} from "../metaSuggest.ts";

let pass = 0;
let fail = 0;
const t = (name: string, fn: () => void) => {
  try {
    fn();
    pass++;
  } catch (e) {
    fail++;
    console.log(`FAIL ${name}: ${(e as Error).message}`);
    process.exitCode = 1;
  }
};

// ── variantLabel ──
t("variantLabel es 1-based", () => {
  assert.equal(variantLabel(0), "v1");
  assert.equal(variantLabel(2), "v3");
});

// ── parseRankingSuggestion: caminos válidos ──
t("ranking válido → bestIndex + order + summary", () => {
  const r = parseRankingSuggestion([1, 0, 2], 3);
  assert.notEqual(r, null);
  assert.equal(r!.bestIndex, 1);
  assert.deepEqual(r!.order, [1, 0, 2]);
  assert.equal(r!.summary, "v2 › v1 › v3");
});

t("ranking de una sola variante", () => {
  const r = parseRankingSuggestion([0], 1);
  assert.notEqual(r, null);
  assert.equal(r!.bestIndex, 0);
  assert.equal(r!.summary, "v1");
});

t("no muta el array de entrada (copia)", () => {
  const input = [2, 1, 0];
  const r = parseRankingSuggestion(input, 3);
  r!.order[0] = 99;
  assert.deepEqual(input, [2, 1, 0]);
});

// ── parseRankingSuggestion: None / OFF / AIE caído ──
t("null → null (feature OFF / AIE caído)", () => {
  assert.equal(parseRankingSuggestion(null, 3), null);
  assert.equal(parseRankingSuggestion(undefined, 3), null);
});

t("array vacío → null", () => {
  assert.equal(parseRankingSuggestion([], 3), null);
});

// ── parseRankingSuggestion: rankings corruptos / desalineados ──
t("longitud != variantCount → null", () => {
  assert.equal(parseRankingSuggestion([0, 1], 3), null);
  assert.equal(parseRankingSuggestion([0, 1, 2], 2), null);
});

t("índice fuera de rango → null", () => {
  assert.equal(parseRankingSuggestion([0, 3, 1], 3), null);
  assert.equal(parseRankingSuggestion([-1, 0, 1], 3), null);
});

t("índice repetido → null", () => {
  assert.equal(parseRankingSuggestion([0, 0, 1], 3), null);
});

t("índice no-entero → null", () => {
  assert.equal(parseRankingSuggestion([0.5, 1, 2], 3), null);
});

t("variantCount <= 0 → null", () => {
  assert.equal(parseRankingSuggestion([0], 0), null);
});

// ── agentCategoryDisplay ──
t("categoría conocida → display", () => {
  assert.equal(agentCategoryDisplay("bugfix"), "bugfix");
  assert.equal(agentCategoryDisplay("perf"), "performance");
  assert.equal(agentCategoryDisplay("style"), "estilo");
});

t("categoría con mayúsculas / espacios se normaliza", () => {
  assert.equal(agentCategoryDisplay("  Bugfix "), "bugfix");
  assert.equal(agentCategoryDisplay("FEATURE"), "feature");
});

t("categoría desconocida razonable se sanea (lower+trim)", () => {
  // Fuera del set conocido pero válida (alfanumérico corto) → se muestra saneada.
  assert.equal(agentCategoryDisplay("Hotfix"), "hotfix");
  assert.equal(agentCategoryDisplay("security-fix"), "security-fix");
});

t("null / vacío → null", () => {
  assert.equal(agentCategoryDisplay(null), null);
  assert.equal(agentCategoryDisplay(undefined), null);
  assert.equal(agentCategoryDisplay("   "), null);
});

// ── HIGH 2 (deepseek): output del AIE fuera del set conocido NO se confía como texto crudo ──
t("categoría fuera del set con basura/inyección → sanitizada, no texto crudo", () => {
  // Caracteres de inyección/markup se descartan; queda sólo alfanumérico + espacio/guion.
  assert.equal(agentCategoryDisplay("<script>alert(1)</script>"), "scriptalert1script");
  assert.equal(agentCategoryDisplay("feat<b>x"), "featbx");
  assert.equal(agentCategoryDisplay("a/b\\c"), "abc");
});

t("categoría larga del AIE se trunca (≤24 chars)", () => {
  const long = "a".repeat(200);
  const out = agentCategoryDisplay(long);
  assert.notEqual(out, null);
  assert.ok(out!.length <= 24, `esperaba ≤24, fue ${out!.length}`);
});

t("categoría que tras sanear queda vacía → descartada (null)", () => {
  assert.equal(agentCategoryDisplay("!!!"), null);
  assert.equal(agentCategoryDisplay("***/\\"), null);
});

// ── HIGH 1: variantsContentKey reacciona a cambios de DIFFS con el mismo count ──
t("variantsContentKey cambia cuando un diff cambia (mismo count)", () => {
  const a = [
    { task_id: "t1", state: "awaiting_review", diff_stat: "1 file +2 -0" },
    { task_id: "t2", state: "awaiting_review", diff_stat: "2 files +5 -1" },
  ];
  const b = [
    { task_id: "t1", state: "awaiting_review", diff_stat: "1 file +9 -3" }, // diff refrescado
    { task_id: "t2", state: "awaiting_review", diff_stat: "2 files +5 -1" },
  ];
  assert.notEqual(variantsContentKey(a), variantsContentKey(b));
});

t("variantsContentKey estable si no cambia nada", () => {
  const a = [{ task_id: "t1", state: "running", diff_stat: "x" }];
  assert.equal(variantsContentKey(a), variantsContentKey([...a]));
});

t("variantsContentKey cambia si cambia el state (mismo diff/count)", () => {
  const a = [{ task_id: "t1", state: "running", diff_stat: "x" }];
  const b = [{ task_id: "t1", state: "awaiting_review", diff_stat: "x" }];
  assert.notEqual(variantsContentKey(a), variantsContentKey(b));
});

// ── HIGH 2 (bestIndex): un ranking de un count distinto al actual se descarta ──
t("ranking de count distinto al actual → descartado (sin bestIndex fuera de rango)", () => {
  // El AIE rankeó 3 variantes pero ahora hay 2 → desalineado → null (no se indexa fuera de rango).
  assert.equal(parseRankingSuggestion([2, 0, 1], 2), null);
  // Caso inverso: rankeó 2, ahora hay 3.
  assert.equal(parseRankingSuggestion([1, 0], 3), null);
});

console.log(`metaSuggest: ${pass} passed, ${fail} failed`);
