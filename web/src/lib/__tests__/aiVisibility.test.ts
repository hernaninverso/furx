// 035-ai-visibility-evidence F1 — tests del núcleo PURO de visibilidad verificable.
// Estilo node:assert (sin DOM, sin vitest). `npm test` lo descubre vía scripts/test-all.mjs.
//
// Cubre la INVARIANTE CENTRAL ("no medido" ≠ 0 y ≠ verde), el veredicto global (partial cuando
// falta una dimensión), el timeline (orden/terminal/normalización TZ), el scrub (bordes) y los
// helpers LIVE (transición provisional + merge canónico-prioritario acotado).

import assert from "node:assert/strict";
import {
  evidenceDimension,
  tokenCostDimension,
  globalVerdict,
  orderBadges,
  fmtTokens,
  toolRowState,
  buildTimeline,
  clampScrub,
  liveStepFromTransition,
  mergeLiveSteps,
  type MeasurementBadge,
  type TimelineStep,
} from "../aiVisibility.ts";
import type { LinterResult, OrchLogEntry, OrchTask, VariantEvidence } from "../../types.ts";

// ── helpers de fixture ──────────────────────────────────────────────────────────────────────
function ev(p: Partial<VariantEvidence>): VariantEvidence {
  return {
    task_id: "t1",
    total_errors: 0,
    total_warnings: 0,
    by_tool: [],
    unavailable_tools: [],
    any_measured: true,
    ...p,
  };
}
function task(p: Partial<OrchTask>): OrchTask {
  return {
    id: "t1", batch_id: "b1", title: "T", objective: "obj",
    repo_path: "/r", branch: "br", state: "running",
    created_at: "2026-06-03 10:00:00", updated_at: "2026-06-03 10:05:00",
    ...p,
  };
}
function log(p: Partial<OrchLogEntry>): OrchLogEntry {
  return { id: "l", task_id: "t1", captured_at: "2026-06-03 10:01:00", source: "poller", content: "x", ...p };
}

// ── 1. evidenceDimension — la INVARIANTE "no medido ≠ 0/verde" ───────────────────────────────
{
  // sin evidencia → unmeasured (NUNCA verde/0).
  const d = evidenceDimension(null);
  assert.equal(d.state, "unmeasured", "sin evidencia debe ser unmeasured");
  assert.notEqual(d.label, "limpio");
  assert.ok(d.measureHint, "unmeasured debe traer hint de cómo medir");
}
{
  // any_measured=false → unmeasured (aunque total_errors=0).
  const d = evidenceDimension(ev({ any_measured: false, total_errors: 0, total_warnings: 0 }));
  assert.equal(d.state, "unmeasured", "any_measured=false NUNCA es 'limpio'");
}
{
  // errores reales → measured_bad.
  const d = evidenceDimension(ev({ total_errors: 2, total_warnings: 1 }));
  assert.equal(d.state, "measured_bad");
  assert.equal(d.measureHint, null);
}
{
  // 0/0 REAL medido → measured_good.
  const d = evidenceDimension(ev({ total_errors: 0, total_warnings: 0 }));
  assert.equal(d.state, "measured_good");
}
{
  // partial-clean (0/0 medido + tools no disponibles) → measured_good PERO partialCoverage=true.
  const d = evidenceDimension(ev({ total_errors: 0, total_warnings: 0, unavailable_tools: ["mypy"] }));
  assert.equal(d.state, "measured_good", "0/0 medido es good aunque falte un tool");
  assert.equal(d.partialCoverage, true, "cobertura parcial marcada (degradará el global)");
  assert.match(d.reason, /sin medir/, "la parcialidad va en el reason (transparencia)");
}
{
  // codex#1: ERRORES reales + tools no disponibles (evidenceBadge kind="partial") → measured_bad,
  // NUNCA measured_good "limpio". El bug era chequear sólo kind==="issues".
  const d = evidenceDimension(ev({ total_errors: 2, total_warnings: 0, unavailable_tools: ["mypy"] }));
  assert.equal(d.state, "measured_bad", "errores reales + tool faltante DEBE ser bad, no limpio");
  assert.equal(d.partialCoverage, true);
  const d2 = evidenceDimension(ev({ total_errors: 0, total_warnings: 3, unavailable_tools: ["ruff"] }));
  assert.equal(d2.state, "measured_bad", "warnings reales + tool faltante también es bad");
}

// ── 2. tokenCostDimension ────────────────────────────────────────────────────────────────────
{
  // sin uso → unmeasured + hint.
  const d = tokenCostDimension(null, "claude");
  assert.equal(d.state, "unmeasured");
  assert.ok(d.measureHint);
}
{
  // uso 0/0 → unmeasured (no inventamos "0 tokens" como medido bueno).
  const d = tokenCostDimension({ input_tokens: 0, output_tokens: 0 }, "claude");
  assert.equal(d.state, "unmeasured");
}
{
  // uso medido (Claude) → measured_good con tokens, USD null implícito (no aparece).
  const d = tokenCostDimension({ input_tokens: 12000, output_tokens: 800, model: "sonnet" }, "claude");
  assert.equal(d.state, "measured_good");
  assert.match(d.label, /tokens/);
  assert.match(d.reason, /Claude/);
}
{
  // CLI no-Claude sin uso → unmeasured con hint específico (multi-agente es follow-up).
  const d = tokenCostDimension(null, "codex");
  assert.equal(d.state, "unmeasured");
  assert.match(d.reason, /codex/);
  assert.match(d.measureHint ?? "", /próxima versión|multi-agente/);
}
{
  // codex#3: NaN/negativo/no-finito → fail-closed a unmeasured (nunca verde con label basura).
  assert.equal(tokenCostDimension({ input_tokens: Number.NaN, output_tokens: 1 }, "claude").state, "unmeasured");
  assert.equal(tokenCostDimension({ input_tokens: -10, output_tokens: 0 }, "claude").state, "unmeasured");
  assert.equal(tokenCostDimension({ input_tokens: Number.POSITIVE_INFINITY, output_tokens: 5 }, "claude").state, "unmeasured");
}

// ── 3. globalVerdict — unmeasured degrada a partial, bad manda ────────────────────────────────
function badge(state: MeasurementBadge["state"], dim = "x"): MeasurementBadge {
  return { dimension: dim, state, label: "", reason: "", measureHint: null };
}
{
  // todo medido y bien → measured_ok.
  const v = globalVerdict([badge("measured_good", "evidence"), badge("measured_good", "tokens")]);
  assert.equal(v.kind, "measured_ok");
  assert.equal(v.unmeasuredDimensions.length, 0);
}
{
  // una dimensión no medida + el resto bien → partial (NUNCA measured_ok). NON-NEGOTIABLE.
  const v = globalVerdict([badge("measured_good", "evidence"), badge("unmeasured", "tokens")]);
  assert.equal(v.kind, "partial", "una dim no medida JAMÁS produce measured_ok");
  assert.deepEqual(v.unmeasuredDimensions, ["tokens"]);
}
{
  // una medida-mal manda aunque haya buenas y no-medidas.
  const v = globalVerdict([badge("measured_bad", "evidence"), badge("measured_good", "tokens"), badge("unmeasured", "x")]);
  assert.equal(v.kind, "measured_issues");
}
{
  // nada se midió → unmeasured.
  const v = globalVerdict([badge("unmeasured", "evidence"), badge("unmeasured", "tokens")]);
  assert.equal(v.kind, "unmeasured");
}
{
  // lista vacía → unmeasured (no measured_ok por vacío).
  assert.equal(globalVerdict([]).kind, "unmeasured");
}
{
  // codex#2: una dim medida-buena PERO con cobertura PARCIAL + otra medida-buena total →
  // NO measured_ok (no "todo medido y limpio" si un tool no se midió). Degrada a partial.
  const v = globalVerdict([
    evidenceDimension(ev({ total_errors: 0, total_warnings: 0, unavailable_tools: ["mypy"] })),
    tokenCostDimension({ input_tokens: 10, output_tokens: 5 }, "claude"),
  ]);
  assert.equal(v.kind, "partial", "cobertura parcial en una dim medida NUNCA da 'todo medido y limpio'");
}
{
  // sanidad: ambas dims medidas-buenas SIN cobertura parcial → sí measured_ok.
  const v = globalVerdict([
    evidenceDimension(ev({ total_errors: 0, total_warnings: 0 })),
    tokenCostDimension({ input_tokens: 10, output_tokens: 5 }, "claude"),
  ]);
  assert.equal(v.kind, "measured_ok");
}

// ── 4. orderBadges / fmtTokens / toolRowState ────────────────────────────────────────────────
{
  const ordered = orderBadges([badge("unmeasured"), badge("measured_good"), badge("measured_bad")]);
  assert.deepEqual(ordered.map((b) => b.state), ["measured_bad", "measured_good", "unmeasured"], "lo malo salta primero, lo no-medido al final");
}
{
  assert.equal(fmtTokens(999), "999");
  assert.equal(fmtTokens(12800), "12.8k");
  assert.equal(fmtTokens(2_500_000), "2.50M");
}
{
  const okClean: LinterResult = { tool: "eslint", status: "ok", errors: 0, warnings: 0, elapsed_ms: 1 };
  const okWarn: LinterResult = { tool: "eslint", status: "ok", errors: 0, warnings: 3, elapsed_ms: 1 };
  const okErr: LinterResult = { tool: "eslint", status: "ok", errors: 1, warnings: 0, elapsed_ms: 1 };
  const na: LinterResult = { tool: "mypy", status: "unavailable", errors: 0, warnings: 0, elapsed_ms: 0 };
  assert.equal(toolRowState(okClean), "measured_good");
  assert.equal(toolRowState(okWarn), "measured_bad");
  assert.equal(toolRowState(okErr), "measured_bad");
  assert.equal(toolRowState(na), "unmeasured", "status != ok NUNCA es 'measured' (ni good ni bad)");
}

// ── 5. buildTimeline — orden, paso terminal, normalización TZ ─────────────────────────────────
{
  // running: spawned + 1 snapshot, sin terminal.
  const tl = buildTimeline(task({ state: "running" }), [log({ captured_at: "2026-06-03 10:02:00" })]);
  assert.equal(tl.length, 2);
  assert.equal(tl[0].kind, "spawned");
  assert.equal(tl[1].kind, "working");
  assert.deepEqual(tl.map((s) => s.index), [0, 1], "re-indexado 0..n");
}
{
  // done: spawned + snapshot + terminal con result_summary.
  const tl = buildTimeline(task({ state: "done", result_summary: "ok!" }), [log({})]);
  assert.equal(tl[tl.length - 1].kind, "terminal");
  assert.equal(tl[tl.length - 1].content, "ok!");
}
{
  // orden ASC aunque los logs vengan desordenados.
  const tl = buildTimeline(task({ state: "running", created_at: "2026-06-03 10:00:00" }), [
    log({ id: "b", captured_at: "2026-06-03 10:03:00", content: "B" }),
    log({ id: "a", captured_at: "2026-06-03 10:01:00", content: "A" }),
  ]);
  const contents = tl.filter((s) => s.kind === "working").map((s) => s.content);
  assert.deepEqual(contents, ["A", "B"], "ordena por captured_at ASC");
}
{
  // ya-ISO (con T y Z) no se rompe.
  const tl = buildTimeline(task({ state: "running", created_at: "2026-06-03T10:00:00Z" }), [
    log({ captured_at: "2026-06-03T10:01:00Z" }),
  ]);
  assert.equal(tl.length, 2);
}

{
  // timestamp NO parseable: NO salta al frente — queda anclado a su predecesor válido (codex#1).
  const tl = buildTimeline(task({ state: "running", created_at: "2026-06-03 10:00:00" }), [
    log({ id: "ok", captured_at: "2026-06-03 10:01:00", content: "OK" }),
    log({ id: "bad", captured_at: "fecha-rota", content: "BAD" }),
  ]);
  const kinds = tl.map((s) => `${s.kind}:${s.content}`);
  // spawned primero (created_at válido), NUNCA el snapshot roto.
  assert.equal(tl[0].kind, "spawned", "el roto no debe saltar antes del spawned");
  assert.ok(kinds.indexOf("working:OK") < kinds.indexOf("working:BAD"), "el roto queda tras su predecesor válido");
}

// ── 6. clampScrub — bordes (incl. NaN fail-closed, codex#2) ──────────────────────────────────
assert.equal(clampScrub(5, 0), 0, "timeline vacío → 0");
assert.equal(clampScrub(-3, 4), 0, "negativo → 0");
assert.equal(clampScrub(10, 4), 3, "fuera de rango → último");
assert.equal(clampScrub(2.9, 4), 2, "trunca a entero");
assert.equal(clampScrub(2, 4), 2);
assert.equal(clampScrub(Number.NaN, 4), 0, "NaN → 0 (fail-closed, nunca NaN)");
assert.equal(clampScrub(Number.POSITIVE_INFINITY, 4), 0, "Inf → 0 (fail-closed)");

// ── 7. liveStepFromTransition — provisional, dedup, sin contenido inventado ───────────────────
{
  // sin transición real (estados iguales) → null.
  assert.equal(liveStepFromTransition("running", "running", "2026-06-03 10:01:00"), null);
}
{
  // transición a estado intermedio → kind "live", provisional, content "".
  const s = liveStepFromTransition("pending", "running", "2026-06-03 10:01:00", "claude");
  assert.ok(s);
  assert.equal(s!.kind, "live");
  assert.equal(s!.provisional, true);
  assert.equal(s!.content, "", "un paso vivo NUNCA fabrica contenido");
  assert.match(s!.label, /claude/);
}
{
  // transición a terminal → kind "terminal" (para que mergeLiveSteps lo dedupe contra el canónico).
  const s = liveStepFromTransition("running", "done", "2026-06-03 10:05:00");
  assert.ok(s);
  assert.equal(s!.kind, "terminal");
  assert.equal(s!.provisional, true);
}
{
  // prev null (primera observación) → emite si next es algo concreto.
  const s = liveStepFromTransition(null, "running", "2026-06-03 10:01:00");
  assert.ok(s);
}

// ── 8. mergeLiveSteps — prioridad canónica, dedup terminal, cap, re-index ─────────────────────
{
  const canonical = buildTimeline(task({ state: "running" }), [log({ captured_at: "2026-06-03 10:01:00" })]);
  const live = [liveStepFromTransition("pending", "running", "2026-06-03 10:02:30")!];
  const merged = mergeLiveSteps(canonical, live);
  assert.equal(merged.length, canonical.length + 1, "appendea el paso vivo");
  assert.deepEqual(merged.map((s) => s.index), merged.map((_, i) => i), "re-indexado contiguo");
  // el paso vivo quedó después del snapshot (10:01) por su timestamp (10:02:30).
  assert.equal(merged[merged.length - 1].provisional, true);
}
{
  // dedup: un paso vivo terminal "done" cuyo estado ya está en el canónico → se descarta.
  const canonical = buildTimeline(task({ state: "done", result_summary: "r" }), [log({})]);
  const liveTerminal = liveStepFromTransition("running", "done", "2026-06-03 10:05:00")!;
  const merged = mergeLiveSteps(canonical, [liveTerminal]);
  const terminals = merged.filter((s) => s.kind === "terminal");
  assert.equal(terminals.length, 1, "el terminal canónico domina al provisional (no doble conteo)");
  assert.ok(!terminals[0].provisional, "el que queda es el canónico, no el provisional");
}
{
  // cap: con muchos pasos, conserva sólo los `cap` más recientes.
  const canonical = buildTimeline(task({ state: "running" }),
    Array.from({ length: 60 }, (_, i) => log({ id: `l${i}`, captured_at: `2026-06-03 10:${String(i % 60).padStart(2, "0")}:00`, content: `c${i}` })));
  const merged = mergeLiveSteps(canonical, [], 50);
  assert.equal(merged.length, 50, "acota a cap");
  assert.deepEqual(merged.map((s) => s.index), merged.map((_, i) => i), "re-indexado tras el cap");
}
{
  // out-of-order: un paso vivo con timestamp anterior se re-ordena estable.
  const canonical = buildTimeline(task({ state: "running", created_at: "2026-06-03 10:00:00" }),
    [log({ captured_at: "2026-06-03 10:05:00", content: "late" })]);
  const early = liveStepFromTransition("pending", "running", "2026-06-03 10:02:00")!;
  const merged = mergeLiveSteps(canonical, [early]);
  const times = merged.map((s) => s.at);
  // spawned(10:00) < live(10:02) < snapshot(10:05)
  assert.ok(times.indexOf("2026-06-03 10:02:00") < times.indexOf("2026-06-03 10:05:00"), "re-ordena out-of-order");
}

// ── 9. ORTOGONALIDAD (FR-012): los pasos vivos NO tocan el veredicto global ───────────────────
{
  // globalVerdict sólo recibe MeasurementBadge (evidencia/costo), nunca TimelineStep — verificado
  // por el tipo. Acá afirmamos que un timeline con pasos provisionales no cambia el veredicto:
  const dims = [badge("measured_good", "evidence"), badge("unmeasured", "tokens")];
  const before = globalVerdict(dims).kind;
  // (un paso vivo existe en otro lado; el veredicto no lo conoce)
  const live: TimelineStep[] = [liveStepFromTransition("pending", "running", "2026-06-03 10:01:00")!];
  void live;
  assert.equal(globalVerdict(dims).kind, before, "el timeline es ortogonal al veredicto");
  assert.equal(before, "partial");
}

console.log("aiVisibility.test.ts: OK");
