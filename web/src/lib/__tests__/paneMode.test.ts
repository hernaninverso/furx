// 022 US8 — tests de la unificación pane.mode ↔ agent_profile_id (lib/paneMode.ts).
// Invariantes:
//  - `synthMode` está en PARIDAD con `synth_mode` de Rust (los 7 casos del unit-test Rust).
//  - `effectivePaneMode` deriva del perfil cuando hay; cae al mode legacy si no (compat).
// `node --experimental-strip-types`. Lo corre scripts/test-all.mjs.
import { synthMode, effectivePaneMode } from "../paneMode.ts";
import type { AgentProfile, PaneCfg } from "../../types.ts";

let pass = 0, fail = 0;
function ok(cond: boolean, name: string) { if (cond) pass++; else { fail++; console.log(`FAIL ${name}`); } }

function profile(over: Partial<AgentProfile> = {}): AgentProfile {
  return {
    id: "p1", name: "Rust reviewer", description: "", cli_kind: "claude",
    account_slug: "trabajo", model: "sonnet", system_prompt: "", default_cwd: null,
    council_enabled: false, council_preset: null, shell_enabled: false, icon: "◆",
    color: null, is_builtin: false, engine_kind: "cli", category: null, plugins: [],
    created_at: "", updated_at: "",
    ...over,
  };
}
function pane(over: Partial<PaneCfg> = {}): PaneCfg {
  return { id: "pane1", mode: "zsh", title: "Pane", ...over } as PaneCfg;
}

// ── synthMode: PARIDAD 1:1 con `agent_profiles.rs::synth_mode` (synth_mode_mapping test). ──
// Casos OK (mirroring los asserts de Rust):
ok(synthMode("zsh", null) === "zsh", "synthMode zsh");
ok(synthMode("claude", "A") === "claude-A", "synthMode claude+A");
ok(synthMode("codex", null) === "codex", "synthMode codex sin slug");
ok(synthMode("codex", "work") === "codex-work", "synthMode codex+work");
ok(synthMode("gemini", null) === "gemini", "synthMode gemini sin slug");
ok(synthMode("aider", null) === "aider", "synthMode aider sin slug");
ok(synthMode("grok", null) === "grok", "synthMode grok sin slug"); // 062
ok(synthMode("grok", "work") === "grok", "synthMode grok ignora slug (OAuth, sin cuentas)"); // 062
ok(synthMode("openai-api", "x") === "openai-api-x", "synthMode openai-api+x");
ok(synthMode("custom", "y") === "custom-y", "synthMode custom+y");
// Casos inválidos → null (en Rust son Err; acá el caller cae al mode legacy):
ok(synthMode("claude", null) === null, "synthMode claude sin cuenta → null");
ok(synthMode("openai-api", null) === null, "synthMode openai-api sin cuenta → null");
ok(synthMode("custom", null) === null, "synthMode custom sin cuenta → null");
ok(synthMode("bogus", null) === null, "synthMode cli_kind desconocido → null");
ok(synthMode("claude", "bad slug") === null, "synthMode slug inválido (espacio) → null");
// slug vacío se trata como ausente (igual que Rust `.filter(|s| !s.is_empty())`):
ok(synthMode("codex", "") === "codex", "synthMode codex slug vacío → legacy");
ok(synthMode("claude", "") === null, "synthMode claude slug vacío → null");

// ── effectivePaneMode ──
const agents = [
  profile({ id: "claude-trabajo", cli_kind: "claude", account_slug: "trabajo" }),
  profile({ id: "codex-default", cli_kind: "codex", account_slug: null }),
  profile({ id: "zsh-prof", cli_kind: "zsh", account_slug: null }),
  profile({ id: "aie-prof", cli_kind: "claude", account_slug: "trabajo", engine_kind: "aie" }),
  profile({ id: "broken-claude", cli_kind: "claude", account_slug: null }), // claude sin cuenta
];

// 1) Sin perfil → el mode legacy manda (compat).
ok(effectivePaneMode(pane({ mode: "zsh" }), agents) === "zsh", "eff sin perfil → zsh legacy");
ok(effectivePaneMode(pane({ mode: "claude-personal" }), agents) === "claude-personal", "eff sin perfil → claude-personal legacy");

// 2) Con perfil que existe → mode derivado del perfil (NO el pane.mode crudo 'zsh').
ok(
  effectivePaneMode(pane({ mode: "zsh", agent_profile_id: "claude-trabajo" }), agents) === "claude-trabajo",
  "eff con perfil claude → claude-trabajo (no zsh)",
);
ok(
  effectivePaneMode(pane({ mode: "zsh", agent_profile_id: "codex-default" }), agents) === "codex",
  "eff con perfil codex sin cuenta → codex",
);
ok(
  effectivePaneMode(pane({ mode: "claude-x", agent_profile_id: "zsh-prof" }), agents) === "zsh",
  "eff con perfil zsh → zsh (override del mode crudo claude-x)",
);

// 3) Perfil con motor 'aie' → NO sintetiza mode CLI; cae al mode legacy.
ok(
  effectivePaneMode(pane({ mode: "zsh", agent_profile_id: "aie-prof" }), agents) === "zsh",
  "eff perfil aie → mode legacy (no synth)",
);

// 4) Perfil referenciado que ya no existe (borrado) → cae al mode legacy (compat, no crash).
ok(
  effectivePaneMode(pane({ mode: "codex", agent_profile_id: "ghost-id" }), agents) === "codex",
  "eff perfil borrado → mode legacy",
);

// 5) Perfil con synth inválido (claude sin cuenta) → cae al mode legacy (no rompe).
ok(
  effectivePaneMode(pane({ mode: "zsh", agent_profile_id: "broken-claude" }), agents) === "zsh",
  "eff perfil claude sin cuenta → mode legacy (synth null)",
);

// ── audit codex (022 US8): restore inicial donde `panes` cargan ANTES que `agents`. ──
// Con agents=[] el perfil no se resuelve → effectivePaneMode cae al pane.mode legacy.
// Cuando agents llega, el effMode pasa de legacy→synth. La React `key` del Terminal
// (Shell.tsx) deriva de effMode, así que DEBE cambiar para forzar un remount limpio que
// re-capture el scrollback/label de la sesión real del spawn.
{
  const p = pane({ mode: "zsh", agent_profile_id: "claude-trabajo" });
  // Antes de que carguen los perfiles (agents vacíos):
  const effBefore = effectivePaneMode(p, []);
  // Después de que llegan los perfiles:
  const effAfter = effectivePaneMode(p, agents);
  ok(effBefore === "zsh", "eff con agents=[] → mode legacy (zsh) [restore temprano]");
  ok(effAfter === "claude-trabajo", "eff con agents poblados → synth (claude-trabajo)");
  ok(effBefore !== effAfter, "effMode CAMBIA cuando los perfiles llegan tarde");

  // La `key` derivada (espeja la de Shell.tsx ~2219) DEBE diferir → remount.
  const keyOf = (eff: string) =>
    `${p.id}::${p.mode}::${eff}::${"" /*cwd*/}::${p.agent_profile_id ?? ""}::${"" /*orch_session*/}`;
  ok(keyOf(effBefore) !== keyOf(effAfter), "la key del Terminal cambia al resolverse el perfil → remount");
}

console.log(`paneMode: ${pass} passed, ${fail} failed`);
process.exit(fail === 0 ? 0 : 1);
