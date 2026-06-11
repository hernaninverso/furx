// 016 US5 (T053 + T074) — tests del gate + allowlist + anti-PII de telemetry. Verifica SC-005.
// `node --experimental-strip-types`. Sink fake (no red); config fija sin Tauri.
import {
  isPropsSafe, looksSecret,
  __setSinkForTest, __setConfigForTest, __resetTelemetryForTest, __sentForTest, __trackForTest,
  type TelemetryEvent,
} from "../telemetry.ts";

let pass = 0, fail = 0;
function ok(cond: boolean, name: string) { if (cond) pass++; else { fail++; console.log(`FAIL ${name}`); } }

// Sink fake: registra lo que se intentaría enviar.
let sunk: TelemetryEvent[] = [];
__setSinkForTest((_ep, ev) => sunk.push(ev));

function reset() { __resetTelemetryForTest(); sunk = []; __setSinkForTest((_ep, ev) => sunk.push(ev)); }

// ── isPropsSafe: allowlist por evento ──────────────────────────────────────────────────────────
ok(isPropsSafe("help_opened", { source: "topbar" }) === true, "help_opened con source válido OK");
ok(isPropsSafe("help_opened", { source: "topbar", extra: "x" }) === false, "prop fuera del schema → DROP");
ok(isPropsSafe("tour_completed", {}) === true, "tour_completed sin props OK");
ok(isPropsSafe("tour_completed", { foo: 1 }) === false, "prop en evento sin props → DROP");
ok(isPropsSafe("command_executed", { category: "work" }) === true, "command_executed category OK");
ok(isPropsSafe("language_changed", { to: "en" }) === true, "language_changed to OK");

// ── Anti-PII: keys prohibidas (T074) ───────────────────────────────────────────────────────────
ok(isPropsSafe("command_executed", { apiKey: "x" } as never) === false, "apiKey → DROP");
ok(isPropsSafe("command_executed", { prompt: "hola" } as never) === false, "prompt → DROP");
ok(isPropsSafe("command_executed", { Authorization: "Bearer x" } as never) === false, "Authorization → DROP");
ok(isPropsSafe("command_executed", { path: "/Users/x" } as never) === false, "path → DROP");
ok(isPropsSafe("command_executed", { email: "a@b.com" } as never) === false, "email → DROP");

// ── Anti-PII: VALORES sospechosos en una key allowlisted (defensa de valor) ─────────────────────
ok(isPropsSafe("command_executed", { category: "sk-ABC123def" }) === false, "valor sk- → DROP");
ok(isPropsSafe("command_executed", { category: "Bearer abc" }) === false, "valor Bearer → DROP");
ok(isPropsSafe("command_executed", { category: "/Users/alice/.furx" }) === false, "valor path → DROP");
ok(isPropsSafe("command_executed", { category: "a@b.com" }) === false, "valor email → DROP");
ok(isPropsSafe("command_executed", { category: "https://x.io/secret" }) === false, "valor url → DROP");
ok(isPropsSafe("command_executed", { category: "eyJhbGc.eyJzdWI.sig" }) === false, "valor JWT → DROP");
ok(isPropsSafe("command_executed", { category: "work" }) === true, "valor categórico limpio OK");

// ── Anti-PII: objetos anidados rechazados ───────────────────────────────────────────────────────
ok(isPropsSafe("command_executed", { category: { nested: true } } as never) === false, "valor objeto → DROP");

// ── M2 (audit): enum/patrón cerrado por campo — NO sólo heurística ───────────────────────────────
// Valores que NO matchean ninguna heurística PII pero igual están fuera del enum cerrado → DROP.
ok(isPropsSafe("language_changed", { to: "fr" }) === false, "M2: idioma fuera del enum es|en → DROP");
ok(isPropsSafe("language_changed", { to: "es" }) === true, "M2: idioma válido es OK");
ok(isPropsSafe("help_opened", { source: "evil" } as never) === false, "M2: source fuera del enum → DROP");
ok(isPropsSafe("command_executed", { category: "tomato sauce" }) === false, "M2: category con espacio (no slug) → DROP");
ok(isPropsSafe("command_executed", { category: "x".repeat(33) }) === false, "M2: category >32 chars → DROP");
ok(isPropsSafe("command_executed", { category: "git-ops" }) === true, "M2: slug categórico válido OK");
ok(isPropsSafe("tour_skipped", { step: 3 }) === true, "M2: step entero acotado OK");
ok(isPropsSafe("tour_skipped", { step: 999 } as never) === false, "M2: step fuera de rango → DROP");
ok(isPropsSafe("tour_skipped", { step: 1.5 } as never) === false, "M2: step no entero → DROP");

// ── looksSecret helper ──────────────────────────────────────────────────────────────────────────
ok(looksSecret("sk-live-abc") === true, "looksSecret sk-");
ok(looksSecret("plain-text") === false, "looksSecret limpio false");

// ── Gate: OFF default → 0 envío (SC-005) ──────────────────────────────────────────────────────
await (async () => {
  reset();
  __setConfigForTest(false, "https://api.furx.cloud/v1"); // opt-in OFF
  await __trackForTest("help_opened", { source: "topbar" });
  ok(__sentForTest().length === 0 && sunk.length === 0, "OFF → 0 eventos enviados");
})();

// ── Gate: ON sin endpoint → descarta ───────────────────────────────────────────────────────────
await (async () => {
  reset();
  __setConfigForTest(true, ""); // ON pero sin endpoint
  await __trackForTest("help_opened", { source: "topbar" });
  ok(sunk.length === 0, "ON sin endpoint → descarta");
})();

// ── Gate: ON + endpoint → envía payload limpio (sin PII/keys) ──────────────────────────────────
await (async () => {
  reset();
  __setConfigForTest(true, "https://api.furx.cloud/v1");
  await __trackForTest("help_opened", { source: "topbar" });
  ok(sunk.length === 1, "ON + endpoint → 1 evento enviado");
  const ev = sunk[0];
  ok(ev.event === "help_opened", "evento correcto");
  ok(JSON.stringify(ev).indexOf("sk-") === -1 && JSON.stringify(ev).toLowerCase().indexOf("apikey") === -1, "payload sin keys");
})();

// ── ON + endpoint pero props con PII inyectada → NO se envía (DROP entero) ──────────────────────
await (async () => {
  reset();
  __setConfigForTest(true, "https://api.furx.cloud/v1");
  await __trackForTest("command_executed", { category: "work", apiKey: "sk-leak" });
  ok(sunk.length === 0, "props con apiKey → NO se emite (DROP entero, SC-005)");
})();

// ── ON + endpoint + valor secreto en campo allowlisted → DROP ──────────────────────────────────
await (async () => {
  reset();
  __setConfigForTest(true, "https://api.furx.cloud/v1");
  await __trackForTest("command_executed", { category: "/Users/alice/secret" });
  ok(sunk.length === 0, "valor path en category → DROP");
})();

console.log(`telemetry: ${pass} passed, ${fail} failed`);
process.exit(fail === 0 ? 0 : 1);
