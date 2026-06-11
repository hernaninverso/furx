// 019 F2 T021 — tests de FormFromSchema: separación rendering/validation/execution-policy +
// anti command-injection. La capa de validación es pura y centralizada; la execution-policy tiene
// allow-list y sólo acepta input branded como validado.
import {
  validate, executeWithPolicy, isValidated,
  type FormSchema, type ExecutionPolicy, type ValidatedInput,
} from "../kit/schemaForm.ts";

let pass = 0, fail = 0;
function ok(cond: boolean, name: string) { if (cond) pass++; else { fail++; console.log(`FAIL ${name}`); } }
async function throws(fn: () => Promise<unknown>, name: string) {
  try { await fn(); fail++; console.log(`FAIL ${name} (no lanzó)`); }
  catch { pass++; }
}

const schema: FormSchema = {
  commandId: "queue_retry",
  fields: [
    { name: "branch", label: "Branch", kind: "text", required: true },
    { name: "count", label: "N", kind: "number", min: 1, max: 4 },
    { name: "agent", label: "Agente", kind: "select", options: [{ value: "codex", label: "Codex" }, { value: "gemini", label: "Gemini" }] },
    { name: "force", label: "Forzar", kind: "boolean" },
    // `note` necesita espacios → opt-out explícito del deny-list anti-shell (el pattern lo acota).
    { name: "note", label: "Nota", kind: "textarea", pattern: /^[\w .,-]*$/, allowShellMeta: true },
  ],
};

// VALIDACIÓN — caso OK.
const good = validate(schema, { branch: "feature-x", count: 2, agent: "codex", force: true, note: "todo ok" });
ok(good.ok && !!good.validated, "input válido pasa y emite validated");
ok(good.errors && Object.keys(good.errors).length === 0, "sin errores en input válido");

// requerido.
ok(!validate(schema, { count: 2 }).ok, "branch requerido faltante → falla");

// ANTI-INJECTION: campo de texto sin pattern propio rechaza metacaracteres de shell.
const inj = validate(schema, { branch: "x; rm -rf /" });
ok(!inj.ok && !!inj.errors.branch, "branch con '; rm -rf' rechazado (shell meta)");
ok(!validate(schema, { branch: "x && curl evil" }).ok, "branch con '&&' rechazado");
ok(!validate(schema, { branch: "$(whoami)" }).ok, "branch con $(...) rechazado");
ok(validate(schema, { branch: "feature-x_1" }).ok, "branch alfanum-guion-guionbajo aceptado");

// number range + select allow-list.
ok(!validate(schema, { branch: "ok", count: 99 }).ok, "count fuera de rango → falla");
ok(!validate(schema, { branch: "ok", agent: "rogue-model" }).ok, "select fuera de allow-list → falla");

// textarea con pattern propio (con allowShellMeta el ';' no lo corta SHELL_META, lo corta el pattern).
ok(!validate(schema, { branch: "ok", note: "drop; table" }).ok, "note viola su pattern → falla");

// HIGH 2 — un `pattern` permisivo NO debe relajar el anti-shell. SHELL_META se aplica SIEMPRE.
const permissive: FormSchema = {
  commandId: "queue_retry",
  fields: [{ name: "branch", label: "B", kind: "text", pattern: /.*/ }], // pattern catch-all
};
ok(!validate(permissive, { branch: "x; rm -rf /" }).ok, "pattern permisivo + '; rm' → rechazado (SHELL_META gana)");
ok(!validate(permissive, { branch: "$(whoami)" }).ok, "pattern permisivo + $(...) → rechazado");
ok(!validate(permissive, { branch: "a | b" }).ok, "pattern permisivo + pipe → rechazado");
ok(!validate(permissive, { branch: "a `id`" }).ok, "pattern permisivo + backticks → rechazado");
ok(validate(permissive, { branch: "feature-x" }).ok, "pattern permisivo + valor limpio → pasa");

// opt-out explícito: allowShellMeta:true sí permite el metacaracter (sólo limitado por el pattern).
const optout: FormSchema = {
  commandId: "queue_retry",
  fields: [{ name: "msg", label: "M", kind: "text", allowShellMeta: true, pattern: /^[\w ;|]*$/ }],
};
ok(validate(optout, { msg: "a; b | c" }).ok, "allowShellMeta:true permite metacaracteres acotados por pattern");

// H2 (audit ronda 2) — `allowShellMeta:true` SIN pattern es config inválida → FALLA siempre (nunca
// queda "todo abierto"). Para permitir un metacaracter hay que proveer un pattern restrictivo.
const noPattern: FormSchema = {
  commandId: "queue_retry",
  fields: [{ name: "msg", label: "M", kind: "text", allowShellMeta: true }], // sin pattern
};
const np = validate(noPattern, { msg: "lo-que-sea" });
ok(!np.ok && !!np.errors.msg, "allowShellMeta:true sin pattern → falla (config inválida)");
// incluso un valor que un metacaracter habría dejado pasar: sigue fallando por config.
ok(!validate(noPattern, { msg: "a; rm -rf /" }).ok, "allowShellMeta:true sin pattern → falla aunque el valor sea peligroso");
// y la misma forma CON pattern acotado sí valida.
const withPattern: FormSchema = {
  commandId: "queue_retry",
  fields: [{ name: "msg", label: "M", kind: "text", allowShellMeta: true, pattern: /^[\w ;]*$/ }],
};
ok(validate(withPattern, { msg: "a; b" }).ok, "allowShellMeta:true CON pattern acotado → valida sólo si matchea");
ok(!validate(withPattern, { msg: "a | b" }).ok, "allowShellMeta:true CON pattern: valor que no matchea el pattern → falla");

// BRAND (HIGH 1) — la marca es INFALSIFICABLE: un objeto fabricado externamente NO pasa, aunque
// sea idéntico en forma, esté frozen, o intente copiar símbolos del objeto real.
const fakeValidated = { commandId: "queue_retry", values: {} } as unknown as ValidatedInput;
ok(!isValidated(fakeValidated), "objeto crudo NO es validated");
ok(!isValidated(Object.freeze({ commandId: "queue_retry", values: Object.freeze({}) })), "objeto frozen idéntico NO es validated");
// intentar extraer/copiar símbolos del objeto validado real no sirve (la marca vive en un WeakSet privado).
const real = good.validated!;
const cloned = { ...(real as object) } as unknown as ValidatedInput;
for (const s of Object.getOwnPropertySymbols(real)) {
  (cloned as Record<symbol, unknown>)[s] = (real as Record<symbol, unknown>)[s];
}
ok(!isValidated(cloned), "clon con símbolos copiados del real NO es validated (marca no está en el objeto)");
ok(isValidated(good.validated!), "el emitido por validate() SÍ es validated");
ok(Object.isFrozen(good.validated!), "el ValidatedInput emitido está frozen");

// EXECUTION-POLICY — allow-list + runner inyectado.
let ran: { cmd: string; values: unknown } | null = null;
const policy: ExecutionPolicy = {
  allow: ["queue_retry"],
  runner: async (cmd, values) => { ran = { cmd, values }; return "ok"; },
};

await (async () => {
  const out = await executeWithPolicy(policy, good.validated!);
  ok(out === "ok" && ran !== null && ran!.cmd === "queue_retry", "policy ejecuta el comando validado vía runner");
})();

// la policy RECHAZA input no validado (defensa en profundidad).
await throws(() => executeWithPolicy(policy, fakeValidated), "policy rechaza input no-branded");

// la policy RECHAZA comandos fuera de su allow-list.
const otherSchema: FormSchema = { commandId: "kill_everything", fields: [] };
const otherValidated = validate(otherSchema, {}).validated!;
await throws(() => executeWithPolicy(policy, otherValidated), "policy rechaza comando fuera de allow-list");

console.log(`kitSchemaForm: ${pass} passed, ${fail} failed`);
process.exit(fail === 0 ? 0 : 1);
