// 022 US2 — tests de la derivación PURA del label de pane (lib/paneLabel.ts).
// Invariante: NUNCA "A"/"B" como label de cuenta; cadena perfil ?? cuenta ?? slug.
// `node --experimental-strip-types`. Lo corre scripts/test-all.mjs.
import { derivePaneLabel, parseMode } from "../paneLabel.ts";
import type { ClaudeAccount, AgentProfile } from "../../types.ts";

let pass = 0, fail = 0;
function ok(cond: boolean, name: string) { if (cond) pass++; else { fail++; console.log(`FAIL ${name}`); } }

function acct(over: Partial<ClaudeAccount> = {}): ClaudeAccount {
  return {
    cli_kind: "claude", slug: "trabajo", label: "Personal", browser: null,
    status: "verified", env_var: null, keychain_service: null,
    last_verified_at: null, last_used_at: null, created_at: "", updated_at: "",
    ...over,
  };
}
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

// parseMode — "openai-api-A" → kind="openai-api" slug="A" (no first-dash).
{
  const r = parseMode("openai-api-A");
  ok(r.kind === "openai-api" && r.slug === "A", "parseMode openai-api-A");
  const c = parseMode("claude-trabajo");
  ok(c.kind === "claude" && c.slug === "trabajo", "parseMode claude-trabajo");
  const z = parseMode("zsh");
  ok(z.kind === "zsh" && z.slug === null, "parseMode zsh sin slug");
}

// 1) 062 — el display deriva del SLUG (nombre real), NO del label arbitrario: aunque la cuenta
//    tenga label "Personal", muestra "Claude Code · trabajo" (el slug). Sin "A/B" suelto.
{
  const accs = [acct({ slug: "trabajo", label: "Personal" })];
  const l = derivePaneLabel("claude-trabajo", accs, []);
  ok(l.label === "Claude Code · trabajo", `label de cuenta (slug): ${l.label}`);
  ok(l.configured === true, "cuenta verificada → configured");
  ok(!/(^|[^a-z])[AB]([^a-z]|$)/.test(l.label) || l.label.includes("trabajo"), "sin A/B suelto");
}

// 2) Sin cuenta configurada → "Claude Code (sin cuenta)", NUNCA "A"/"B".
{
  const l = derivePaneLabel("claude-A", [], []);
  ok(l.label === "Claude Code (sin cuenta)", `sin cuenta: ${l.label}`);
  ok(l.configured === false, "sin cuenta → !configured (accionable)");
  ok(l.label !== "Claude · A" && l.label !== "A" && l.label !== "B", "label nunca es A/B");
}

// 3) Agent-profile presente → usa su name (máxima prioridad de la cadena).
{
  const accs = [acct({ slug: "trabajo", label: "Personal" })];
  const l = derivePaneLabel("claude-trabajo", accs, [profile()], "p1");
  ok(l.label === "Rust reviewer", `perfil gana: ${l.label}`);
  ok(l.sublabel.includes("sonnet"), `sublabel con modelo: ${l.sublabel}`);
  ok(l.icon === "◆", "ícono del perfil");
}

// 4) Fallback chain: perfil inexistente → cae a cuenta.
{
  const accs = [acct({ slug: "trabajo", label: "Personal" })];
  const l = derivePaneLabel("claude-trabajo", accs, [], "fantasma");
  ok(l.label === "Claude Code · trabajo", `perfil inexistente cae a cuenta (slug): ${l.label}`);
}

// 5) missing_token → estado honesto, accionable, no placeholder.
{
  const accs = [acct({ slug: "trabajo", label: "Personal", status: "missing_token" })];
  const l = derivePaneLabel("claude-trabajo", accs, []);
  ok(l.configured === false, "missing_token → !configured");
  ok(l.sublabel.includes("falta token"), `sublabel falta token: ${l.sublabel}`);
}

// 6) zsh → terminal.
{
  const l = derivePaneLabel("zsh", [], []);
  ok(l.label === "zsh", "zsh label");
}

// 7) CLI legacy sin slug (codex auth default).
{
  const l = derivePaneLabel("codex", [], []);
  ok(l.label === "Codex CLI (auth default)", `codex legacy: ${l.label}`);
}

console.log(`paneLabel: ${pass} passed, ${fail} failed`);
process.exit(fail === 0 ? 0 : 1);
