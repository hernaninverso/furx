// spec-022 US2 — Derivación PURA del label de un pane. CERO hardcode "A/B".
//
// El label se deriva en CADENA: `agentProfile.name ?? cuenta.label ?? slug legible`.
// Nunca un literal "A"/"B" suelto. Si la cuenta no está configurada, el estado es
// claro y accionable ("Claude (sin cuenta)"), no un placeholder falso.
//
// Lógica pura y testeable: no toca Tauri ni React. Shell la usa para el header del
// pane y el dropdown de modos (una sola fuente de verdad del label).

import type { ClaudeAccount, AgentProfile } from "../types.ts";
import { CLI_KIND_META } from "../types.ts";

/** Lo que necesita el header/dropdown del pane para pintarse, todo derivado. */
export interface PaneLabel {
  /** Glifo corto (1-2 chars o emoji del perfil). */
  icon: string;
  /** Color del acento (token V3 o color del perfil). */
  color: string;
  /** Nombre derivado: perfil → cuenta → slug legible. */
  label: string;
  /** Línea secundaria: cli_kind + modelo / estado de cuenta. Vacío si no aplica. */
  sublabel: string;
  /** false cuando es un pane Claude/CLI-con-slug SIN cuenta real configurada. */
  configured: boolean;
}

const KIND_PREFIXES = [
  "openai-api-",
  "claude-",
  "codex-",
  "gemini-",
  "aider-",
  "custom-",
] as const;

/** Parsea `pane.mode` en `{ kind, slug }`. "openai-api-A" → kind="openai-api" slug="A". */
export function parseMode(mode: string): { kind: string; slug: string | null } {
  for (const p of KIND_PREFIXES) {
    if (mode.startsWith(p)) {
      return { kind: p.slice(0, -1), slug: mode.slice(p.length) };
    }
  }
  return { kind: mode, slug: null };
}

/** Capitaliza un slug crudo para mostrarlo si no hay mejor fuente (ej. "trabajo" → "Trabajo"). */
function prettySlug(slug: string): string {
  const s = slug.replace(/[-_]+/g, " ").trim();
  if (!s) return slug;
  return s.charAt(0).toUpperCase() + s.slice(1);
}

function kindLabel(kind: string): string {
  return (CLI_KIND_META as Record<string, { label: string }>)[kind]?.label ?? kind;
}
function kindColor(kind: string): string {
  return (CLI_KIND_META as Record<string, { color: string }>)[kind]?.color ?? "var(--indigo)";
}

/**
 * Deriva el label de un pane SIN literales "A/B".
 *
 * @param mode  pane.mode ("zsh" | "claude-<slug>" | "codex" | ...)
 * @param claudeAccounts  cuentas CLI reales del usuario
 * @param agents  agent-profiles guardados (built-in + del user)
 * @param agentProfileId  si el pane referencia un perfil, su id (tiene prioridad)
 */
export function derivePaneLabel(
  mode: string,
  claudeAccounts: ClaudeAccount[],
  agents: AgentProfile[],
  agentProfileId?: string | null,
): PaneLabel {
  // 1) Perfil (máxima prioridad de la cadena).
  if (agentProfileId) {
    const prof = agents.find((a) => a.id === agentProfileId);
    if (prof) {
      const acc =
        prof.account_slug != null
          ? claudeAccounts.find((a) => a.cli_kind === prof.cli_kind && a.slug === prof.account_slug)
          : undefined;
      const parts = [kindLabel(prof.cli_kind)];
      if (prof.model) parts.push(prof.model);
      else if (acc) parts.push(acc.slug);
      return {
        icon: prof.icon || "◆",
        color: prof.color || kindColor(prof.cli_kind),
        label: prof.name,
        sublabel: parts.join(" · "),
        configured: true,
      };
    }
    // El perfil referenciado ya no existe → caemos a derivar del mode.
  }

  // 2) Terminal puro.
  if (mode === "zsh") {
    return { icon: ">_", color: "#6c7b91", label: "zsh", sublabel: "", configured: true };
  }

  const { kind, slug } = parseMode(mode);

  // 3) CLI con slug → cuenta real.
  if (slug) {
    const acc = claudeAccounts.find((a) => a.cli_kind === kind && a.slug === slug);
    if (acc) {
      const statusNote =
        acc.status === "verified"
          ? ""
          : acc.status === "missing_token"
            ? " · falta token"
            : " · sin verificar";
      return {
        icon: acc.slug.slice(0, 2).toUpperCase(),
        color:
          acc.status === "verified"
            ? kindColor(kind)
            : acc.status === "missing_token"
              ? "var(--red)"
              : "var(--amber)",
        label: `${kindLabel(kind)} · ${acc.slug}`,
        sublabel: `${kind}${statusNote}`,
        configured: acc.status !== "missing_token",
      };
    }
    // Cuenta NO configurada: estado honesto y accionable, NUNCA "A"/"B".
    return {
      icon: kindLabel(kind).slice(0, 1).toUpperCase(),
      color: "var(--amber)",
      label: `${kindLabel(kind)} (sin cuenta)`,
      sublabel: prettySlug(slug),
      configured: false,
    };
  }

  // 4) CLI legacy sin slug (auth default del CLI / env vars existentes).
  if (CLI_KIND_META[kind as keyof typeof CLI_KIND_META]) {
    return {
      icon: kind.slice(0, 2).toUpperCase(),
      color: kindColor(kind),
      label: `${kindLabel(kind)} (auth default)`,
      sublabel: kind,
      configured: true,
    };
  }

  // 5) Fallback genérico (modos no-terminal __data__/__web__/… los maneja el caller).
  return { icon: "?", color: "var(--indigo)", label: mode, sublabel: "", configured: true };
}
