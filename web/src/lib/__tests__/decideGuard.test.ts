// 044 FR-003 — tests de la lógica PURA del guard anti doble-respuesta (`node --experimental-strip-types`).
// seq POR-CARD (audit-3 fix): dos cards distintas NO se invalidan entre sí.
import { createSeqState, beginInvoke, shouldApply, DECIDE_TIMEOUT_MS } from "../decideGuard.ts";

let pass = 0, fail = 0;
function ok(cond: boolean, name: string) { if (cond) pass++; else { fail++; console.log(`FAIL ${name}`); } }

// seq por card arranca en 0 implícito; cada beginInvoke incrementa SU contador.
{
  const s = createSeqState();
  const a1 = beginInvoke(s, "A");
  ok(a1 === 1, "1er beginInvoke de A → 1");
  const a2 = beginInvoke(s, "A");
  ok(a2 === 2, "2do beginInvoke de A → 2");
  const b1 = beginInvoke(s, "B");
  ok(b1 === 1, "1er beginInvoke de B → 1 (independiente de A)");
}

// shouldApply: sólo el ÚLTIMO seq de ESA card aplica.
{
  const s = createSeqState();
  const a1 = beginInvoke(s, "A");
  ok(shouldApply(s, "A", a1), "el único de A en vuelo aplica");
  const a2 = beginInvoke(s, "A");
  ok(!shouldApply(s, "A", a1), "resolución del seq VIEJO de A (post-timeout) NO aplica");
  ok(shouldApply(s, "A", a2), "resolución del seq NUEVO de A sí aplica");
}

// audit-3 CRÍTICO: A y B son independientes — arrancar B NO invalida la acción en vuelo de A.
{
  const s = createSeqState();
  const a1 = beginInvoke(s, "A");
  const b1 = beginInvoke(s, "B"); // arranca B
  ok(shouldApply(s, "A", a1), "arrancar B NO invalida a A (seq por-card)");
  ok(shouldApply(s, "B", b1), "B también vigente");
}

// audit-3: el TIMEOUT consume el seq (un nuevo beginInvoke) → la resolución posterior del mismo
// invoke queda stale (evita doble-efecto error→éxito).
{
  const s = createSeqState();
  const a1 = beginInvoke(s, "A"); // invoke arranca, seq=1
  // timeout vence → consume el seq:
  beginInvoke(s, "A"); // seq=2 (consumido por el timeout)
  ok(!shouldApply(s, "A", a1), "tras el timeout (seq consumido), la resolución tardía del invoke NO aplica");
}

ok(DECIDE_TIMEOUT_MS === 15000, "timeout de re-habilitación = 15s");

console.log(`decideGuard: ${pass} passed, ${fail} failed`);
process.exit(fail === 0 ? 0 : 1);
