// 019 F2 T020 — tests de la lógica PURA de selección/lote del kit (`lib/kit/selection.ts`).
import {
  toggle, toggleAll, pruneSelection, partitionEligible,
  startBatch, advance, isComplete, pct,
} from "../kit/selection.ts";

let pass = 0, fail = 0;
function ok(cond: boolean, name: string) { if (cond) pass++; else { fail++; console.log(`FAIL ${name}`); } }

// toggle.
ok(toggle(new Set(), "a").has("a"), "toggle agrega");
ok(!toggle(new Set(["a"]), "a").has("a"), "toggle saca");
// inmutable: no muta el set original.
const s0 = new Set(["a"]);
toggle(s0, "b");
ok(s0.size === 1, "toggle no muta el original");

// toggleAll.
ok(toggleAll(new Set(), ["a", "b"]).size === 2, "toggleAll vacío → selecciona todos");
ok(toggleAll(new Set(["a", "b"]), ["a", "b"]).size === 0, "toggleAll lleno → limpia");
ok(toggleAll(new Set(["a"]), ["a", "b"]).size === 2, "toggleAll parcial → selecciona todos");
ok(toggleAll(new Set(), []).size === 0, "toggleAll lista vacía → vacío");

// pruneSelection: saca fantasmas.
ok([...pruneSelection(new Set(["a", "ghost"]), ["a", "b"])].join() === "a", "prune saca ids ausentes");

// partitionEligible: retry sólo sobre failed.
const states: Record<string, string> = { a: "failed", b: "done", c: "failed" };
const part = partitionEligible(new Set(["a", "b", "c"]), (id) => states[id] === "failed");
ok(part.actionable.sort().join() === "a,c", "actionable = los failed");
ok(part.blocked.join() === "b", "blocked = el done");

// progreso.
let p = startBatch(3);
ok(pct(p) === 0 && !isComplete(p), "batch inicial 0% incompleto");
p = advance(p, true); p = advance(p, false);
ok(p.done === 2 && p.errors === 1, "advance cuenta done+errors");
ok(pct(p) === 67, "pct redondea 2/3");
p = advance(p, true);
ok(isComplete(p) && pct(p) === 100, "completo al llegar al total");
ok(pct(startBatch(0)) === 0, "pct con total 0 no divide por cero");

console.log(`kitSelection: ${pass} passed, ${fail} failed`);
process.exit(fail === 0 ? 0 : 1);
