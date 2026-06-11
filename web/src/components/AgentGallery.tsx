// 006 agent-profiles — "Agent Gallery": CRUD de perfiles de agente + export/import.
// Estética V3 "atelier" (tokens de design/furx-theme.css). BYOK: nunca toca tokens;
// el account_slug sólo referencia una cuenta local (el backend resuelve el Keychain).
import { useEffect, useMemo, useState } from "react";
import { invoke } from "../lib/invoke"; // 015 T015: invoke con flujo de aprobación universal
import type { AgentProfile, ClaudeAccount } from "../types";
// 022 US11 / FR-016 — editor de capabilities por perfil (lógica pura).
import {
  grantBadges, canActivate, isPluginActive, togglePlugin, validatePlugins,
  pluginKind, pluginKindLabel, type InstalledPlugin, type GrantTone,
} from "../lib/capabilityEditor";

/** Forma cruda del Plugin de `plugins_list` (permissions van anidados en manifest). */
interface RawPlugin {
  id: string;
  name: string;
  version: string;
  enabled: boolean;
  verified: boolean;
  manifest: { name: string; version: string; description?: string | null; commands?: string[]; permissions?: string[] };
  installed_at?: string;
}

/** Plugin instalado normalizado para el editor (permissions aplanados al top-level). */
interface PluginListItem extends InstalledPlugin {
  id: string;
}

/** Aplana el Plugin crudo de `plugins_list` al shape que consume la lógica pura. */
function normalizePlugin(p: RawPlugin): PluginListItem {
  return {
    id: p.id,
    name: p.name,
    version: p.version,
    enabled: p.enabled,
    verified: p.verified,
    permissions: p.manifest?.permissions ?? [],
  };
}

/** Color del badge de grant según su tono (tokens V3, dark+light). */
function grantColor(tone: GrantTone): { fg: string; bg: string; bd: string } {
  switch (tone) {
    case "danger": return { fg: "var(--clay, #b8543a)", bg: "var(--clay-pale, rgba(184,84,58,.10))", bd: "var(--clay, #b8543a)" };
    case "warn":   return { fg: "var(--amber, #9a6b1e)", bg: "var(--amber-pale, rgba(154,107,30,.10))", bd: "var(--amber, #9a6b1e)" };
    case "info":   return { fg: "var(--accent)", bg: "var(--accent-glow)", bd: "var(--accent)" };
    default:       return { fg: "var(--ink-dim, #6b6358)", bg: "var(--line, rgba(0,0,0,.05))", bd: "var(--line, rgba(0,0,0,.18))" };
  }
}

/** Chip del tipo de plugin (MCP / Memoria de código / Herramienta). */
function kindColor(kind: ReturnType<typeof pluginKind>): { fg: string; bg: string } {
  if (kind === "mcp") return { fg: "var(--accent)", bg: "var(--accent-glow)" };
  if (kind === "codebase-memory") return { fg: "var(--plum, #6b3f7a)", bg: "rgba(107,63,122,.12)" };
  return { fg: "var(--ink-dim, #6b6358)", bg: "var(--line, rgba(0,0,0,.06))" };
}

const CLI_KINDS = ["zsh", "claude", "codex", "gemini", "aider", "grok", "openai-api", "custom"] as const;
// CLIs que requieren una cuenta (slug) sí o sí (no tienen modo legacy sin cuenta).
const REQUIRES_ACCOUNT = new Set(["claude", "openai-api", "custom"]);

type Draft = Partial<AgentProfile> & { name: string; cli_kind: string };

const EMPTY: Draft = {
  name: "", description: "", cli_kind: "claude", account_slug: null, model: "",
  system_prompt: "", default_cwd: "", council_enabled: false, shell_enabled: false, plugins: [],
  engine_kind: "cli", category: null,
};

export function AgentGallery({
  open, onClose, agents, accounts, onChanged, onToast, activeAgentIds,
}: {
  open: boolean;
  onClose: () => void;
  agents: AgentProfile[];
  accounts: ClaudeAccount[];
  onChanged: () => void;
  onToast: (kind: "success" | "error" | "info", msg: string) => void;
  /** 047 FR-003 — ids de agente que están corriendo en algún pane (borde teal + badge "activo"). */
  activeAgentIds?: Set<string>;
}) {
  const [draft, setDraft] = useState<Draft>(EMPTY);
  const [editingId, setEditingId] = useState<string | null>(null);
  const [importText, setImportText] = useState("");
  const [busy, setBusy] = useState(false);
  // 022 US11 — plugins instalados (fuente: plugins_list, disco = SoT).
  const [installed, setInstalled] = useState<PluginListItem[]>([]);
  const [pluginsErr, setPluginsErr] = useState<string | null>(null);
  const [savingCaps, setSavingCaps] = useState(false);

  // Reset el editor cuando se abre.
  useEffect(() => { if (open) { setDraft(EMPTY); setEditingId(null); setImportText(""); } }, [open]);

  // 022 US11 — cargar los plugins instalados al abrir (para el selector de capabilities).
  useEffect(() => {
    if (!open) return;
    invoke<RawPlugin[]>("plugins_list")
      .then((ps) => { setInstalled(ps.map(normalizePlugin)); setPluginsErr(null); })
      .catch((e) => { setInstalled([]); setPluginsErr(String(e)); });
  }, [open]);

  const accountsForKind = useMemo(
    () => accounts.filter((a) => a.cli_kind === draft.cli_kind),
    [accounts, draft.cli_kind],
  );

  if (!open) return null;

  const isBuiltinEditing = editingId ? agents.find((a) => a.id === editingId)?.is_builtin : false;

  const loadInto = (a: AgentProfile) => {
    setEditingId(a.id);
    setDraft({ ...a });
  };

  // Clonar un built-in (preset de rol): carga sus campos como un agente NUEVO editable
  // (editingId=null → save crea). El user le asigna su cuenta y guarda.
  const cloneInto = (a: AgentProfile) => {
    setEditingId(null);
    setDraft({ ...a, id: undefined, is_builtin: false, name: `${a.name} (copia)` });
  };

  const save = async () => {
    // No correr un save mientras un toggle de capability persiste (mandaría otro
    // agent_profile_update con un `plugins` potencialmente distinto → race / pérdida).
    if (busy || savingCaps) return;
    if (!draft.name.trim()) { onToast("error", "El agente necesita un nombre."); return; }
    // Para el motor 'aie' la cuenta es opcional (el bearer sale del Keychain; default
    // 'aie-internal-bearer'). La requisitoria de cuenta sólo aplica al motor 'cli'.
    if (draft.engine_kind !== "aie" && REQUIRES_ACCOUNT.has(draft.cli_kind) && !draft.account_slug) {
      onToast("error", `Un agente ${draft.cli_kind} necesita una cuenta.`); return;
    }
    // Invariante: nunca persistir un nombre de plugin que no exista en disco. `draft.plugins`
    // puede venir de un perfil existente/importado/clonado/stale con nombres arbitrarios o de
    // plugins ya desinstalados → filtrar contra `plugins_list` (igual que el toggle), así TODO
    // camino de persistencia (toggle, save, create) chequea contra el disco.
    const installedNames = installed.map((x) => x.name);
    const profile: Partial<AgentProfile> = {
      ...draft,
      id: editingId ?? "",
      account_slug: draft.account_slug || null,
      model: (draft.model || "").trim() || null,
      default_cwd: (draft.default_cwd || "").trim() || null,
      engine_kind: draft.engine_kind || "cli",
      category: (draft.category || "").trim() || null,
      plugins: validatePlugins(draft.plugins ?? [], installedNames),
      council_enabled: !!draft.council_enabled,
      shell_enabled: !!draft.shell_enabled,
      description: draft.description ?? "",
      system_prompt: draft.system_prompt ?? "",
    };
    setBusy(true);
    try {
      if (editingId) await invoke("agent_profile_update", { profile });
      else await invoke("agent_profile_create", { profile });
      onToast("success", editingId ? "Agente actualizado." : "Agente creado.");
      setDraft(EMPTY); setEditingId(null); onChanged();
    } catch (e) { onToast("error", `No se pudo guardar: ${String(e)}`); }
    finally { setBusy(false); }
  };

  // 022 US11 / FR-016 — togglear una capability (plugin/MCP/codebase-memory) en el
  // perfil. Escribe el array `plugins` de forma inmutable. Si el perfil YA existe
  // (editingId), persiste de inmediato vía `agent_profile_update` → el backend dispara
  // la inyección MCP (`mcp_inject`) y el indexado del repo (`codebase_index`). Si es un
  // perfil nuevo sin guardar, sólo se stagea en el draft hasta el "Crear agente".
  const toggleCapability = async (p: PluginListItem) => {
    // Serializar contra save/delete/import (busy) y contra otro toggle en vuelo
    // (savingCaps): un update concurrente con otro payload de `plugins` perdería este
    // cambio. Si hay un write en progreso, ignorar el toggle (la UI ya lo deshabilita).
    if (busy || savingCaps) return;
    const act = canActivate(p);
    const currentlyActive = isPluginActive(draft.plugins, p.name);
    // No se puede ACTIVAR un plugin no-activable (global-disabled / sin firma).
    // Sí se permite DESACTIVAR uno ya activo aunque haya quedado no-activable.
    if (!act.activatable && !currentlyActive) {
      onToast("error", act.reason ?? "Plugin no activable.");
      return;
    }
    // Defensivo: el array que persistimos sólo contiene nombres de plugins que EXISTEN
    // en disco (de plugins_list). Nunca escribimos un name arbitrario en agent_profile.
    const installedNames = installed.map((x) => x.name);
    const nextPlugins = validatePlugins(togglePlugin(draft.plugins, p.name), installedNames);
    setDraft((d) => ({ ...d, plugins: nextPlugins }));

    // Perfil nuevo (sin id) → se persiste al guardar, no ahora.
    if (!editingId) {
      onToast("info", currentlyActive
        ? `«${p.name}» se quitará al crear el agente.`
        : `«${p.name}» se activará al crear el agente.`);
      return;
    }

    // Perfil existente → persistir ya (dispara mcp_inject / codebase_index).
    setSavingCaps(true);
    const base = agents.find((a) => a.id === editingId);
    try {
      const profile: Partial<AgentProfile> = {
        ...(base ?? {}), ...draft, id: editingId,
        account_slug: draft.account_slug || null,
        plugins: nextPlugins,
      };
      await invoke("agent_profile_update", { profile });
      const kindLbl = pluginKindLabel(pluginKind(p));
      onToast("success", currentlyActive
        ? `«${p.name}» desactivado en este perfil.`
        : `«${p.name}» (${kindLbl}) activado. El .mcp.json se generará al abrir un pane con este perfil.`);
      onChanged();
    } catch (e) {
      // revertir el toggle optimista si falló la persistencia.
      setDraft((d) => ({ ...d, plugins: draft.plugins ?? [] }));
      onToast("error", `No se pudo guardar la capability: ${String(e)}`);
    } finally { setSavingCaps(false); }
  };

  const del = async (a: AgentProfile) => {
    if (busy || savingCaps) return;
    if (a.is_builtin) { onToast("error", "Los agentes built-in no se borran."); return; }
    setBusy(true);
    try {
      await invoke("agent_profile_delete", { id: a.id });
      onToast("success", "Agente eliminado.");
      if (editingId === a.id) { setDraft(EMPTY); setEditingId(null); }
      onChanged();
    } catch (e) { onToast("error", `No se pudo borrar: ${String(e)}`); }
    finally { setBusy(false); }
  };

  const exportOne = async (a: AgentProfile) => {
    try {
      const json = await invoke<unknown>("agent_profile_export", { id: a.id });
      const text = JSON.stringify(json, null, 2);
      await navigator.clipboard.writeText(text).catch(() => {});
      onToast("success", `"${a.name}" exportado al portapapeles (sin secretos).`);
    } catch (e) { onToast("error", `Export falló: ${String(e)}`); }
  };

  const importNow = async () => {
    let parsed: unknown;
    try { parsed = JSON.parse(importText); }
    catch { onToast("error", "JSON inválido."); return; }
    setBusy(true);
    try {
      await invoke("agent_profile_import", { json: parsed });
      onToast("success", "Agente importado. Asignale una cuenta local y guardá.");
      setImportText(""); onChanged();
    } catch (e) { onToast("error", `Import falló: ${String(e)}`); }
    finally { setBusy(false); }
  };

  const lbl: React.CSSProperties = { fontFamily: "var(--mono)", fontSize: 11, letterSpacing: ".06em", textTransform: "uppercase", color: "var(--ink-dim, #6b6358)", display: "block", margin: "10px 0 3px" };
  const inp: React.CSSProperties = { width: "100%", background: "var(--bg, #faf7f0)", color: "var(--ink, #1c1814)", border: "1px solid var(--line, rgba(0,0,0,.15))", borderRadius: 6, padding: "7px 9px", fontFamily: "var(--body)", fontSize: 14 };

  return (
    <div role="dialog" aria-label="Gestión de agentes" onClick={onClose}
      style={{ position: "fixed", inset: 0, background: "rgba(0,0,0,.45)", zIndex: 400, display: "flex", alignItems: "center", justifyContent: "center" }}>
      <div onClick={(e) => e.stopPropagation()}
        style={{ width: "min(1080px,96vw)", maxHeight: "88vh", overflow: "hidden", display: "grid", gridTemplateColumns: "minmax(420px, 1.4fr) 1fr",
                 background: "var(--bg, #f3efe6)", color: "var(--ink, #1c1814)", border: "1px solid var(--line, rgba(0,0,0,.18))", borderRadius: 10, boxShadow: "0 20px 60px -20px rgba(0,0,0,.5)" }}>
        {/* 047 FR-003 — galería de agentes en grid de 3 columnas (responsive). El agente que está
            corriendo en algún pane lleva borde teal + badge "activo". El seleccionado en el editor
            queda resaltado (coral-pale). Borde coral = activo es ORTOGONAL a la selección. */}
        <div style={{ borderRight: "1px solid var(--line, rgba(0,0,0,.12))", overflowY: "auto", padding: 14 }}>
          <div style={{ fontFamily: "var(--display, serif)", fontSize: 20, fontWeight: 600, marginBottom: 10 }}>Agentes</div>
          <button onClick={() => { setDraft(EMPTY); setEditingId(null); }}
            style={{ ...inp, cursor: "pointer", textAlign: "left", marginBottom: 10, fontWeight: 600 }}>+ Nuevo agente</button>
          <div role="list" aria-label="Galería de agentes"
            style={{ display: "grid", gridTemplateColumns: "repeat(auto-fill, minmax(120px, 1fr))", gap: 8 }}>
            {agents.map((a) => {
              const selected = editingId === a.id;
              const active = !!activeAgentIds?.has(a.id);
              return (
                <div key={a.id} role="listitem" onClick={() => loadInto(a)}
                  aria-current={selected ? "true" : undefined}
                  style={{ padding: "9px 10px", borderRadius: 8, cursor: "pointer",
                           display: "flex", flexDirection: "column", gap: 4, minWidth: 0,
                           background: selected ? "var(--accent-glow)" : "var(--bg2, rgba(0,0,0,.02))",
                           border: active ? "2px solid var(--accent)"
                             : selected ? "1px solid var(--accent)"
                             : "1px solid var(--line, rgba(0,0,0,.12))",
                           boxShadow: active ? "0 0 0 1px var(--accent) inset" : undefined }}>
                  <div style={{ display: "flex", alignItems: "center", gap: 6, minWidth: 0 }}>
                    <span style={{ fontSize: 14, fontWeight: 600, overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>
                      {a.icon ? a.icon + " " : "◆ "}{a.name}
                    </span>
                    {active && (
                      <span title="Corriendo en un panel"
                        style={{ marginLeft: "auto", flexShrink: 0, fontFamily: "var(--mono)", fontSize: 9, fontWeight: 700,
                                 color: "var(--accent)", border: "1px solid var(--accent)", borderRadius: 4, padding: "1px 5px" }}>
                        activo
                      </span>
                    )}
                  </div>
                  <div style={{ fontFamily: "var(--mono)", fontSize: 10, color: "var(--ink-dim, #6b6358)", overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>
                    {a.cli_kind}{a.account_slug ? ` · ${a.account_slug}` : ""}{a.is_builtin ? " · built-in" : ""}
                  </div>
                </div>
              );
            })}
          </div>
        </div>
        {/* Editor */}
        <div style={{ overflowY: "auto", padding: 18 }}>
          <div style={{ display: "flex", justifyContent: "space-between", alignItems: "center" }}>
            <div style={{ fontFamily: "var(--display, serif)", fontSize: 20, fontWeight: 600 }}>
              {editingId ? (isBuiltinEditing ? "Preset built-in (read-only)" : "Editar agente") : "Nuevo agente"}
            </div>
            <div style={{ display: "flex", gap: 8 }}>
              {isBuiltinEditing && editingId && (
                <button onClick={() => { const a = agents.find((x) => x.id === editingId); if (a) cloneInto(a); }}
                  style={{ ...inp, width: "auto", cursor: "pointer", padding: "4px 12px", background: "var(--accent)", color: "#fff", border: "none", fontWeight: 600 }}>Clonar para usar</button>
              )}
              <button onClick={onClose} aria-label="Cerrar" style={{ ...inp, width: "auto", cursor: "pointer", padding: "4px 10px" }}>×</button>
            </div>
          </div>

          <label style={lbl}>Nombre</label>
          <input style={inp} value={draft.name} disabled={!!isBuiltinEditing}
            onChange={(e) => setDraft({ ...draft, name: e.target.value })} placeholder="Rust reviewer" />

          <label style={lbl}>CLI</label>
          <select style={inp} value={draft.cli_kind} disabled={!!isBuiltinEditing}
            onChange={(e) => setDraft({ ...draft, cli_kind: e.target.value, account_slug: null })}>
            {CLI_KINDS.map((k) => <option key={k} value={k}>{k}</option>)}
          </select>

          <label style={lbl}>Motor</label>
          <select style={inp} value={draft.engine_kind ?? "cli"} disabled={!!isBuiltinEditing}
            onChange={(e) => setDraft({ ...draft, engine_kind: e.target.value })}>
            <option value="cli">CLI (corre en el pane)</option>
            <option value="aie">AIE / HTTP (chat REPL en el pane)</option>
          </select>

          <label style={lbl}>Categoría (opcional)</label>
          <input style={inp} value={draft.category ?? ""} disabled={!!isBuiltinEditing}
            onChange={(e) => setDraft({ ...draft, category: e.target.value })} placeholder="soporte / ventas / qa…" />

          {/* 062 — grok NO es account-managed (usa su propio `grok login`/OAuth) → sin selector de
              cuenta (sino se podría setear un account_slug → "grok-<slug>" que no existe en resolve_mode). */}
          {draft.cli_kind !== "zsh" && draft.cli_kind !== "grok" && (
            <>
              <label style={lbl}>Cuenta{REQUIRES_ACCOUNT.has(draft.cli_kind) ? " (requerida)" : " (opcional)"}</label>
              <select style={inp} value={draft.account_slug ?? ""} disabled={!!isBuiltinEditing}
                onChange={(e) => setDraft({ ...draft, account_slug: e.target.value || null })}>
                <option value="">{REQUIRES_ACCOUNT.has(draft.cli_kind) ? "— elegí una cuenta —" : "default del CLI"}</option>
                {/* 062 — el display de la cuenta es el SLUG (nombre real), no el label arbitrario. */}
                {accountsForKind.map((a) => <option key={a.slug} value={a.slug}>{a.slug}</option>)}
              </select>
              {accountsForKind.length === 0 && REQUIRES_ACCOUNT.has(draft.cli_kind) && (
                <div style={{ fontSize: 12, color: "var(--clay, #b8543a)", marginTop: 4 }}>
                  No tenés cuentas {draft.cli_kind}. Agregá una en Cuentas primero.
                </div>
              )}
            </>
          )}

          <label style={lbl}>Modelo (opcional)</label>
          <input style={inp} value={draft.model ?? ""} disabled={!!isBuiltinEditing}
            onChange={(e) => setDraft({ ...draft, model: e.target.value })} placeholder="sonnet / opus / gpt-5…" />

          <label style={lbl}>Instrucciones / system-prompt</label>
          <textarea style={{ ...inp, minHeight: 90, fontFamily: "var(--mono)", fontSize: 13 }} value={draft.system_prompt ?? ""}
            disabled={!!isBuiltinEditing} onChange={(e) => setDraft({ ...draft, system_prompt: e.target.value })}
            placeholder="Sos un experto en Rust. Respondé conciso…" />
          {draft.cli_kind !== "claude" && draft.cli_kind !== "aider" && (draft.system_prompt || draft.model) && (
            <div style={{ fontSize: 12, color: "var(--ink-dim, #6b6358)", marginTop: 4 }}>
              Nota: modelo/instrucciones se guardan, pero en v1 sólo se inyectan a Claude/Aider.
            </div>
          )}

          <label style={lbl}>cwd por defecto (opcional)</label>
          <input style={inp} value={draft.default_cwd ?? ""} disabled={!!isBuiltinEditing}
            onChange={(e) => setDraft({ ...draft, default_cwd: e.target.value })} placeholder="~/proj (dentro de $HOME o /tmp)" />

          <label style={{ ...lbl, display: "flex", alignItems: "center", gap: 8, textTransform: "none", letterSpacing: 0, fontSize: 14, fontFamily: "var(--body)" }}>
            <input type="checkbox" checked={!!draft.council_enabled} disabled={!!isBuiltinEditing}
              onChange={(e) => setDraft({ ...draft, council_enabled: e.target.checked })} /> Disponible como voz del Council
          </label>

          {/* 022 US11 / FR-016 — capabilities por perfil: plugins / MCP / memoria de código. */}
          {!isBuiltinEditing && (
            <div style={{ marginTop: 18, paddingTop: 14, borderTop: "1px solid var(--line, rgba(0,0,0,.12))" }}>
              <label style={lbl}>Plugins / MCP / Memoria</label>
              <div style={{ fontSize: 12, color: "var(--ink-dim, #6b6358)", margin: "0 0 8px" }}>
                Activá capabilities para este perfil. Al activar, el agente obtiene el plugin (los servidores MCP se inyectan en el <code>.mcp.json</code> al abrir un pane con este perfil). Los badges muestran qué le otorgás antes de activar.
              </div>

              {pluginsErr && (
                <div style={{ fontSize: 12, color: "var(--clay, #b8543a)", marginBottom: 6 }}>No se pudieron cargar los plugins: {pluginsErr}</div>
              )}
              {!pluginsErr && installed.length === 0 && (
                <div style={{ fontSize: 13, color: "var(--ink-dim, #6b6358)", padding: "8px 0" }}>
                  Sin plugins instalados. Instalá uno desde la vista Plugins (bundle firmado) y volvé acá.
                </div>
              )}

              <div style={{ display: "flex", flexDirection: "column", gap: 6 }}>
                {installed.map((p) => {
                  const active = isPluginActive(draft.plugins, p.name);
                  const act = canActivate(p);
                  const blocked = !act.activatable && !active;
                  const kind = pluginKind(p);
                  const kc = kindColor(kind);
                  const badges = grantBadges(p.permissions ?? []);
                  return (
                    <div key={p.id || p.name}
                      style={{ border: active ? "1px solid var(--accent)" : "1px solid var(--line, rgba(0,0,0,.14))",
                               borderRadius: 8, padding: "9px 10px", background: active ? "var(--accent-glow)" : "transparent",
                               opacity: blocked ? 0.55 : 1 }}>
                      <label style={{ display: "flex", alignItems: "flex-start", gap: 9, cursor: blocked ? "not-allowed" : "pointer" }}>
                        <input type="checkbox" checked={active} disabled={blocked || savingCaps}
                          aria-label={`Activar ${p.name} en este perfil`}
                          onChange={() => void toggleCapability(p)} style={{ marginTop: 3 }} />
                        <div style={{ flex: 1, minWidth: 0 }}>
                          <div style={{ display: "flex", alignItems: "center", gap: 7, flexWrap: "wrap" }}>
                            <span style={{ fontWeight: 600, fontSize: 14 }}>{p.name}</span>
                            <span style={{ fontFamily: "var(--mono)", fontSize: 11, color: "var(--ink-dim, #6b6358)" }}>v{p.version}</span>
                            <span style={{ fontSize: 10, fontWeight: 600, padding: "1px 7px", borderRadius: 10, color: kc.fg, background: kc.bg }}>
                              {pluginKindLabel(kind)}
                            </span>
                            {!p.verified && (
                              <span title="firma Ed25519 ausente/inválida — el motor rehúsa ejecutarlo"
                                style={{ fontSize: 10, fontWeight: 600, padding: "1px 7px", borderRadius: 10, color: "var(--clay, #b8543a)", background: "var(--clay-pale, rgba(184,84,58,.10))" }}>
                                sin firma válida
                              </span>
                            )}
                          </div>
                          {/* Badges de grant BYOK: qué le otorgás al activar (transparencia + gobierno). */}
                          {badges.length > 0 && (
                            <div style={{ display: "flex", gap: 5, flexWrap: "wrap", marginTop: 6 }}>
                              {badges.map((b) => {
                                const c = grantColor(b.tone);
                                return (
                                  <span key={b.key} title={b.title}
                                    style={{ fontSize: 11, padding: "2px 8px", borderRadius: 10, color: c.fg, background: c.bg, border: `1px solid ${c.bd}` }}>
                                    <span aria-hidden="true">{b.icon}</span> {b.label}
                                  </span>
                                );
                              })}
                            </div>
                          )}
                          {badges.length === 0 && (
                            <div style={{ fontSize: 11, color: "var(--ink-dim, #6b6358)", marginTop: 5 }}
                              title="El manifest no declara ningún permiso sensible (todo permiso declarado, conocido o no, se muestra como badge arriba). Sólo se omiten marcadores que no son grants: «mcp» y «net: ninguno».">
                              Sin permisos sensibles declarados.
                            </div>
                          )}
                          {blocked && act.reason && (
                            <div style={{ fontSize: 11, color: "var(--clay, #b8543a)", marginTop: 5 }}>{act.reason}</div>
                          )}
                        </div>
                      </label>
                    </div>
                  );
                })}
              </div>
            </div>
          )}

          {!isBuiltinEditing && (
            <div style={{ display: "flex", gap: 8, marginTop: 16 }}>
              <button onClick={save} disabled={busy || savingCaps}
                style={{ ...inp, width: "auto", cursor: "pointer", background: "var(--accent)", color: "#fff", border: "none", fontWeight: 600, padding: "8px 16px", opacity: busy || savingCaps ? 0.6 : 1 }}>
                {editingId ? "Guardar cambios" : "Crear agente"}
              </button>
              {editingId && (
                <>
                  <button onClick={() => { const a = agents.find((x) => x.id === editingId); if (a) void exportOne(a); }}
                    style={{ ...inp, width: "auto", cursor: "pointer", padding: "8px 14px" }}>Exportar</button>
                  <button onClick={() => { const a = agents.find((x) => x.id === editingId); if (a) void del(a); }} disabled={busy || savingCaps}
                    style={{ ...inp, width: "auto", cursor: "pointer", color: "var(--clay, #b8543a)", padding: "8px 14px" }}>Eliminar</button>
                </>
              )}
            </div>
          )}

          {/* Import */}
          <div style={{ marginTop: 22, paddingTop: 14, borderTop: "1px solid var(--line, rgba(0,0,0,.12))" }}>
            <label style={lbl}>Importar agent.json (sin secretos — asignás tu cuenta local)</label>
            <textarea style={{ ...inp, minHeight: 60, fontFamily: "var(--mono)", fontSize: 12 }} value={importText}
              onChange={(e) => setImportText(e.target.value)} placeholder='{"schema":"furx.agent-profile.v1", …}' />
            <button onClick={importNow} disabled={busy || !importText.trim()}
              style={{ ...inp, width: "auto", cursor: "pointer", marginTop: 6, padding: "7px 14px" }}>Importar</button>
          </div>
        </div>
      </div>
    </div>
  );
}
