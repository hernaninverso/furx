// 016 US4 (T043 + T077) — tests de la FSM del tour (PURA). Avance/back/skip, target ausente con
// presupuesto ACOTADO (no loop), terminales, persistencia con guards. `node --experimental-strip-types`.
import {
  tourReducer, initialState, visibleSteps, shouldFallbackAdvance, WAIT_BUDGET, isFirstRunDone,
  type TourStep,
} from "../tour.ts";

let pass = 0, fail = 0;
function ok(cond: boolean, name: string) { if (cond) pass++; else { fail++; console.log(`FAIL ${name}`); } }

const TOTAL = 3;

// ── Estado inicial ─────────────────────────────────────────────────────────────────────────────
let s = initialState();
ok(s.status === "idle" && s.index === 0, "inicial: idle index 0");

// ── start → running ─────────────────────────────────────────────────────────────────────────────
s = tourReducer(s, { type: "start" });
ok(s.status === "running" && s.index === 0 && s.waitBudget === WAIT_BUDGET, "start → running, budget lleno");

// ── next avanza; en el último → completed ─────────────────────────────────────────────────────
s = tourReducer(s, { type: "next", total: TOTAL });
ok(s.status === "running" && s.index === 1, "next → index 1");
s = tourReducer(s, { type: "next", total: TOTAL });
ok(s.status === "running" && s.index === 2, "next → index 2 (último)");
s = tourReducer(s, { type: "next", total: TOTAL });
ok(s.status === "completed", "next en último → completed");

// ── back retrocede; no baja de 0 ─────────────────────────────────────────────────────────────
let b = tourReducer(initialState(), { type: "start" });
b = tourReducer(b, { type: "next", total: TOTAL });
b = tourReducer(b, { type: "back" });
ok(b.index === 0, "back → index 0");
b = tourReducer(b, { type: "back" });
ok(b.index === 0, "back no baja de 0");

// ── skip → skipped (terminal) ─────────────────────────────────────────────────────────────────
const sk = tourReducer(tourReducer(initialState(), { type: "start" }), { type: "skip" });
ok(sk.status === "skipped", "skip → skipped");
// acciones tras terminal no cambian estado.
ok(tourReducer(sk, { type: "next", total: TOTAL }).status === "skipped", "next tras skipped = no-op");

// ── target ausente: presupuesto ACOTADO, sin loop infinito ─────────────────────────────────────
let w = tourReducer(initialState(), { type: "start" });
// martillamos targetMissing más veces que el presupuesto.
let guard = 0;
while (!shouldFallbackAdvance(w) && guard < WAIT_BUDGET + 5) {
  w = tourReducer(w, { type: "targetMissing" });
  guard++;
}
ok(w.status === "waitingTarget", "targetMissing → waitingTarget");
ok(shouldFallbackAdvance(w), "presupuesto agotado → fallback advance (no loop)");
ok(guard <= WAIT_BUDGET + 1, `presupuesto ACOTADO (${guard} ticks <= ${WAIT_BUDGET + 1})`);
// targetFound resetea a running y rellena el presupuesto.
const found = tourReducer(w, { type: "targetFound" });
ok(found.status === "running" && found.waitBudget === WAIT_BUDGET, "targetFound → running, budget lleno");

// ── visibleSteps filtra por flag (requiresFlag OFF se excluye) ─────────────────────────────────
const steps: TourStep[] = [
  { id: "a", targetId: "ta", domain: "X", titleKey: "tour.next", bodyKey: "tour.skip" },
  { id: "b", targetId: "tb", domain: "X", titleKey: "tour.next", bodyKey: "tour.skip", requiresFlag: "__nope__" },
];
const vis = visibleSteps(steps);
ok(vis.length === 1 && vis[0].id === "a", "visibleSteps excluye paso con flag OFF/inexistente");

// ── persistencia con guards: sin localStorage, isFirstRunDone no rompe ─────────────────────────
ok(typeof isFirstRunDone() === "boolean", "isFirstRunDone devuelve boolean sin romper (guard)");

console.log(`tour: ${pass} passed, ${fail} failed`);
process.exit(fail === 0 ? 0 : 1);
