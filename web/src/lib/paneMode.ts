// spec-022 US8 — Unificación pane.mode ↔ agent_profile_id.
//
// Un pane puede referenciar un agent-profile (006) como su FUENTE DE VERDAD de qué
// corre. Cuando lo hace, el `mode` legacy se DERIVA del perfil (cli_kind + account_slug)
// en vez de quedar desacoplado. El backend ya hace esto en el spawn real
// (`pty_spawn` → `resolve_agent_runtime` → `synth_mode`); acá replicamos la pieza pura
// `synth_mode` en TS para que la chrome (captura de scrollback + label de sesión tmux)
// apunte a la MISMA sesión que el backend crea, sin divergencia.
//
// IMPORTANTE: el spawn canónico SIEMPRE pasa por el `synth_mode` de Rust — esta función
// sólo decide qué tmux session mostramos/restauramos (best-effort, cosmético). Mantenerla
// en PARIDAD con `src-tauri/src/services/agent_profiles.rs::synth_mode` (hay un test que
// enumera los 7 casos). Lógica pura: no toca Tauri ni React.

import type { AgentProfile, PaneCfg } from "../types.ts";

/** Slug válido: 1-32 chars [A-Za-z0-9_-]. Espeja `valid_slug` de Rust. */
function validSlug(slug: string): boolean {
  return slug.length > 0 && slug.length <= 32 && /^[A-Za-z0-9_-]+$/.test(slug);
}

/**
 * Mapea (cli_kind, account_slug) al `mode` string que entiende el spawn (`resolve_mode`).
 * Réplica EXACTA de `synth_mode` (Rust). Pura → unit-testeable.
 *
 * Devuelve `null` (en vez de lanzar) cuando la combinación es inválida (ej. claude sin
 * cuenta, slug malformado, cli_kind desconocido): el caller cae al `pane.mode` legacy.
 */
export function synthMode(cliKind: string, accountSlug: string | null | undefined): string | null {
  const slug = accountSlug && accountSlug.length > 0 ? accountSlug : null;
  if (slug !== null && !validSlug(slug)) return null;

  switch (cliKind) {
    case "zsh":
      return "zsh";
    // Claude SIEMPRE necesita cuenta (no hay mode legacy "claude").
    case "claude":
      return slug !== null ? `claude-${slug}` : null;
    // Tienen mode legacy sin slug (usan la config/env default del CLI).
    case "codex":
    case "gemini":
    case "aider":
      return slug !== null ? `${cliKind}-${slug}` : cliKind;
    // 062 — grok NO es account-managed (OAuth propio): SIEMPRE "grok", ignora cualquier slug (no hay
    // grok-<slug> en resolve_mode → caería a zsh).
    case "grok":
      return "grok";
    // Sólo existen como "<kind>-<slug>" en resolve_mode.
    case "openai-api":
      return slug !== null ? `openai-api-${slug}` : null;
    case "custom":
      return slug !== null ? `custom-${slug}` : null;
    default:
      return null;
  }
}

/**
 * El `mode` EFECTIVO de un pane = lo que el backend realmente correrá:
 *  - si el pane referencia un agent-profile (006) que existe y es motor `cli`, se deriva
 *    del perfil (cli_kind + account_slug) vía `synthMode` (SSOT con el spawn de Rust);
 *  - en cualquier otro caso (sin perfil, perfil borrado, motor `aie`, o synth inválido)
 *    se cae al `pane.mode` legacy → retrocompat total con panes existentes.
 *
 * @param pane    el pane (mode legacy + opcional agent_profile_id)
 * @param agents  los agent-profiles cargados (built-in + del usuario)
 */
export function effectivePaneMode(
  pane: Pick<PaneCfg, "mode" | "agent_profile_id">,
  agents: AgentProfile[],
): string {
  const pid = pane.agent_profile_id;
  if (pid) {
    const prof = agents.find((a) => a.id === pid);
    // El motor 'aie' NO usa una sesión tmux por mode (es un REPL HTTP) — su scrollback
    // se restaura por el path del pane normal; no sintetizamos un mode CLI para él.
    if (prof && prof.engine_kind !== "aie") {
      const m = synthMode(prof.cli_kind, prof.account_slug);
      if (m) return m;
    }
    // Perfil ausente/borrado o synth inválido → caemos al mode legacy (compat).
  }
  return pane.mode;
}
