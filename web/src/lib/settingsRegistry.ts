// US7 — Settings registry tipado + search (spec 015-frontend-reform-kernel).
//
// Mirror TS del registry curado en Rust (`src-tauri/src/services/settings_registry.rs`).
// El front carga la lista de `SettingDef` vía el comando Tauri
// `settings_registry_list` y genera la Settings UI (tabs + search + controls) a
// partir de ella. Escribe valores vía `settings_set_validated`, que valida
// contra el schema en el backend antes de persistir.
//
// BYOK: las API keys NUNCA son settings — viven en el Keychain. El dominio
// `Accounts` referencia metadata/estado, nunca el secreto.

import { invoke } from "@tauri-apps/api/core";

export type SettingDomain =
  | "Accounts"
  | "Agents"
  | "Plugins"
  | "Audio"
  | "Signals"
  | "Orchestration"
  | "Review"
  | "Appearance"
  | "Shortcuts"
  | "Advanced"
  | "Memory";

export type Visibility = "Visible" | "Advanced" | "Internal";

export type Risk = "Safe" | "Caution" | "Destructive";

/** Discriminated union mirroring `SettingSchema` (serde tag = "type"). */
export type SettingSchema =
  | { type: "bool" }
  | { type: "string"; max_len: number | null }
  | { type: "enum"; options: string[] }
  | { type: "number"; min: number | null; max: number | null };

export interface SettingDef {
  key: string;
  domain: SettingDomain;
  label: string;
  description: string;
  default_value: unknown;
  schema: SettingSchema;
  visibility: Visibility;
  restart_required: boolean;
  risk: Risk;
}

/** Order in which domain tabs render (spec US7 enumeration). */
export const DOMAIN_ORDER: SettingDomain[] = [
  "Accounts",
  "Agents",
  "Plugins",
  "Memory",
  "Audio",
  "Signals",
  "Orchestration",
  "Review",
  "Appearance",
  "Shortcuts",
  "Advanced",
];

export const DOMAIN_LABELS: Record<SettingDomain, string> = {
  Accounts: "Accounts & BYOK",
  Agents: "Agents & Presets",
  Plugins: "Plugins & Permissions",
  Memory: "Memoria",
  Audio: "Audio & Voice",
  Signals: "Signals & Remote",
  Orchestration: "Orchestration",
  Review: "Review & Safety",
  Appearance: "Appearance",
  Shortcuts: "Shortcuts",
  Advanced: "Advanced",
};

/** Load the curated registry from the Rust backend. */
export async function loadRegistry(): Promise<SettingDef[]> {
  return invoke<SettingDef[]>("settings_registry_list");
}

/** Persist a value, schema-validated server-side. Throws on invalid value. */
export async function setValidated(key: string, value: unknown): Promise<void> {
  await invoke("settings_set_validated", { key, value });
}

/** Read all persisted settings as a key→value map (existing KV store). */
export async function loadValues(): Promise<Record<string, unknown>> {
  const pairs = await invoke<Array<[string, unknown]>>("settings_all");
  return Object.fromEntries(pairs);
}

/**
 * Client-side schema validation — mirrors the Rust `SettingSchema::validate`.
 * Used for instant UI feedback; the backend validates authoritatively. Returns
 * `null` if valid, or a user-facing error message.
 */
export function validateValue(schema: SettingSchema, value: unknown): string | null {
  switch (schema.type) {
    case "bool":
      return typeof value === "boolean" ? null : "expected a boolean";
    case "string": {
      if (typeof value !== "string") return "expected a string";
      if (schema.max_len != null && [...value].length > schema.max_len) {
        return `string too long (max ${schema.max_len} chars)`;
      }
      return null;
    }
    case "enum": {
      if (typeof value !== "string") return "expected one of the allowed options";
      return schema.options.includes(value)
        ? null
        : `'${value}' is not one of: ${schema.options.join(", ")}`;
    }
    case "number": {
      if (typeof value !== "number" || Number.isNaN(value)) return "expected a number";
      if (schema.min != null && value < schema.min) return `below minimum ${schema.min}`;
      if (schema.max != null && value > schema.max) return `above maximum ${schema.max}`;
      return null;
    }
    default:
      return null;
  }
}

/**
 * Filter the registry by a free-text query across key, label and description.
 * Case-insensitive substring match. Empty query → all settings.
 */
export function searchSettings(defs: SettingDef[], query: string): SettingDef[] {
  const q = query.trim().toLowerCase();
  if (!q) return defs;
  return defs.filter(
    (d) =>
      d.key.toLowerCase().includes(q) ||
      d.label.toLowerCase().includes(q) ||
      d.description.toLowerCase().includes(q),
  );
}
