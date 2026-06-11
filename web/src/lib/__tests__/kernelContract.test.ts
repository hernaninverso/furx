// 016 (T079) — contract tests del KERNEL que la Fase 1.5 reusa. Garantiza que las superficies nuevas
// (Help/What's New/tours/telemetry) NO rompen invariantes del kernel (015). `node --experimental-strip-types`.
//
// No podemos cargar el registry Rust en node (no hay runtime Tauri), así que validamos los contratos
// ESTÁTICOS testeables: unicidad de rutas/deeplinks, enums válidos en los CommandDef sintéticos,
// monotonicidad del eventBus, y que Help no duplica entradas sobre un fixture.
import { navTargets, parseRoute, parseModalRoute, parseHelpRoute, isWhatsNewRoute, VIEWS, SETTINGS_SECTIONS, MODALS } from "../router.ts";
import { applyEnvelope, __resetForTest, lastSeenSeq, type EventEnvelope } from "../eventBus.ts";
import { buildHelpIndex, __resetHelpMemo } from "../help.ts";
import type { CommandDef } from "../commandRegistry.ts";

let pass = 0, fail = 0;
function ok(cond: boolean, name: string) { if (cond) pass++; else { fail++; console.log(`FAIL ${name}`); } }

const SCOPES = ["app", "window", "view", "pane"];
const RISKS = ["safe", "destructive", "credential", "external"];
const VIS = ["primary", "palette", "internal", "hidden"];

// ── 1) navTargets: rutas ÚNICAS y todas parseables (view|modal|help|whatsnew) ──────────────────
const targets = navTargets();
const routes = targets.map((t) => t.route);
ok(new Set(routes).size === routes.length, "rutas de navTargets únicas");
ok(
  targets.every((t) => parseRoute(t.route) || parseModalRoute(t.route) || parseHelpRoute(t.route) || isWhatsNewRoute(t.route)),
  "toda navTarget parsea por algún parser del router",
);
ok(targets.length === VIEWS.length + SETTINGS_SECTIONS.length + MODALS.length + 2, "navTargets count estable (+help +whatsnew)");
// VIEWS/MODALS/SETTINGS_SECTIONS sin duplicados (SSOT).
ok(new Set(VIEWS).size === VIEWS.length, "VIEWS sin duplicados");
ok(new Set(MODALS).size === MODALS.length, "MODALS sin duplicados");

// ── 2) Las rutas nuevas (help/whatsnew) NO colisionan con vistas ───────────────────────────────
ok(parseRoute("furx://help") === null, "furx://help NO es una vista (no colisiona)");
ok(parseRoute("furx://whatsnew") === null, "furx://whatsnew NO es una vista");
ok(parseHelpRoute("furx://help") !== null, "furx://help parsea como Help");

// ── 3) eventBus: seq MONOTÓNICO (un envelope viejo no pisa uno nuevo). ──────────────────────────
__resetForTest();
const ev = (seq: number): EventEnvelope => ({ tag: "CommandExecuted", data: { command_id: "x" }, seq, ts: 0 });
ok(applyEnvelope(ev(1)) === true && lastSeenSeq() === 1, "aplica seq 1");
ok(applyEnvelope(ev(2)) === true && lastSeenSeq() === 2, "aplica seq 2");
ok(applyEnvelope(ev(2)) === false && lastSeenSeq() === 2, "descarta replay exacto (seq 2)");
ok(applyEnvelope(ev(1)) === false && lastSeenSeq() === 2, "descarta seq viejo (1)");
ok(applyEnvelope(ev(5)) === true && lastSeenSeq() === 5, "aplica salto a 5");
__resetForTest();

// ── 4) Help NO duplica comandos (ids únicos en el índice). ─────────────────────────────────────
function cmd(over: Partial<CommandDef>): CommandDef {
  return { id: "x", label: "X", description: "", category: "work", scope: "app", risk: "safe",
    visibility: "palette", shortcut: null, requires_confirmation: false, reversible: true,
    deeplink: null, extra: {}, ...over };
}
__resetHelpMemo();
const fixture: CommandDef[] = [
  cmd({ id: "a", label: "A" }),
  cmd({ id: "b", label: "B", category: "intelligence" }),
  cmd({ id: "c", label: "C", visibility: "hidden" }),
];
// Validar enums del fixture (espejo del contrato Rust — el real lo testea cargo, acá el shape).
for (const c of fixture) {
  ok(SCOPES.includes(c.scope), `scope válido (${c.id})`);
  ok(RISKS.includes(c.risk), `risk válido (${c.id})`);
  ok(VIS.includes(c.visibility), `visibility válida (${c.id})`);
}
const index = buildHelpIndex(fixture);
const ids = index.map((e) => e.id);
ok(new Set(ids).size === ids.length, "Help: ids del índice ÚNICOS (no duplica comandos)");
ok(!ids.includes("c"), "Help: excluye hidden del índice");
__resetHelpMemo();

console.log(`kernelContract: ${pass} passed, ${fail} failed`);
process.exit(fail === 0 ? 0 : 1);
