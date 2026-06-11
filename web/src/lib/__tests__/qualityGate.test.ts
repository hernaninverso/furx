// 024-quality-gate F1 — tests de la lógica pura de presentación de evidencia.
// Estilo node:assert (sin DOM, sin vitest). `npm test` lo descubre vía scripts/test-all.mjs.
import assert from "node:assert/strict";
import {
  evidenceBadge,
  evidenceByTask,
  isMeasured,
  statusLabel,
  toolCellText,
} from "../qualityGate.ts";
import type { LinterResult, VariantEvidence } from "../../types.ts";

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

const ok = (tool: string, errors: number, warnings: number): LinterResult => ({
  tool,
  status: "ok",
  errors,
  warnings,
  issues: [],
  reason: null,
  raw_excerpt: null,
  elapsed_ms: 1,
});
const unavailable = (tool: string, reason: string): LinterResult => ({
  tool,
  status: "unavailable",
  errors: 0,
  warnings: 0,
  issues: [],
  reason,
  raw_excerpt: null,
  elapsed_ms: 1,
});

const ev = (over: Partial<VariantEvidence>): VariantEvidence => ({
  task_id: "t1",
  total_errors: 0,
  total_warnings: 0,
  by_tool: [],
  unavailable_tools: [],
  any_measured: false,
  ...over,
});

// ── INVARIANTE CENTRAL: "no disponible" NUNCA es 0 ──
t("linter ausente → 'no disponible', NUNCA '0'", () => {
  const r = unavailable("eslint", "binario no encontrado");
  const text = toolCellText(r);
  assert.ok(text.includes("no disponible"), `esperaba 'no disponible', got '${text}'`);
  assert.ok(!/\b0\b/.test(text), `NUNCA debe renderizar un 0 para un linter no medido: '${text}'`);
  assert.equal(isMeasured(r), false);
});

t("variante sin NINGUNA medición → badge 'unavailable' (no 'limpio'/'0')", () => {
  const b = evidenceBadge(ev({ any_measured: false, unavailable_tools: ["clippy", "ruff"] }));
  assert.ok(b);
  assert.equal(b!.kind, "unavailable");
  assert.equal(b!.label, "no disponible");
  assert.ok(!/\b0\b/.test(b!.label));
});

// ── Caso medido limpio (0 REAL) ──
t("variante medida con 0/0 → 'limpio' (un 0 REAL, no falso)", () => {
  const b = evidenceBadge(ev({ any_measured: true, total_errors: 0, total_warnings: 0 }));
  assert.equal(b!.kind, "clean");
  assert.equal(b!.label, "limpio");
});

t("linter ok con 0/0 → 'limpio'", () => {
  assert.equal(toolCellText(ok("clippy", 0, 0)), "limpio");
});

// ── Conteos ──
t("badge con errores y warnings", () => {
  const b = evidenceBadge(ev({ any_measured: true, total_errors: 2, total_warnings: 3 }));
  assert.equal(b!.kind, "issues");
  assert.equal(b!.errors, 2);
  assert.equal(b!.warnings, 3);
  assert.ok(b!.label.includes("2 errores"));
  assert.ok(b!.label.includes("3 warnings"));
});

t("singular/plural en el label", () => {
  const b = evidenceBadge(ev({ any_measured: true, total_errors: 1, total_warnings: 1 }));
  assert.ok(b!.label.includes("1 error"));
  assert.ok(b!.label.includes("1 warning"));
});

t("toolCellText con errores muestra 'N err · M warn'", () => {
  assert.equal(toolCellText(ok("clippy", 2, 1)), "2 err · 1 warn");
  assert.equal(toolCellText(ok("clippy", 0, 5)), "5 warn");
  assert.equal(toolCellText(ok("clippy", 3, 0)), "3 err");
});

// ── Parcial: medido + algún tool no disponible (transparencia) ──
t("medido limpio pero con un tool no disponible → 'partial'", () => {
  const b = evidenceBadge(
    ev({ any_measured: true, total_errors: 0, total_warnings: 0, unavailable_tools: ["mypy"] }),
  );
  assert.equal(b!.kind, "partial");
  assert.deepEqual(b!.unavailableTools, ["mypy"]);
});

// ── statusLabel + null ──
t("statusLabel cubre todos los estados", () => {
  assert.equal(statusLabel("ok"), "ok");
  assert.equal(statusLabel("unavailable"), "no disponible");
  assert.equal(statusLabel("timeout"), "tiempo agotado");
  assert.equal(statusLabel("unparsable"), "salida no interpretable");
});

t("evidenceBadge(null) → null (estado vacío sin error)", () => {
  assert.equal(evidenceBadge(null), null);
  assert.equal(evidenceBadge(undefined), null);
});

// ── Index por task ──
t("evidenceByTask indexa por task_id", () => {
  const m = evidenceByTask([ev({ task_id: "a" }), ev({ task_id: "b" })]);
  assert.equal(m.size, 2);
  assert.ok(m.has("a"));
  assert.ok(m.has("b"));
  assert.equal(evidenceByTask(null).size, 0);
});

console.log(`qualityGate: ${pass} passed, ${fail} failed`);
