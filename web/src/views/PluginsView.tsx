// spec-kit 001 · US2/US3 — Plugins view.
// Lists installed plugins (registry), enable/disable, and an invoke panel that
// runs a plugin tool out-of-process (signature-verified, permission-gated by the
// Rust host). V3 atelier tokens via styles.css classes.

import { useCallback, useEffect, useState } from "react";
import { invoke } from "../lib/invoke"; // 015 T015: invoke con flujo de aprobación universal
import { pluginLabel, pluginDescription } from "../lib/pluginDisplay"; // 022 MED 2: label del name sanitizado, description texto plano truncado
import { Button } from "../components/Button";

interface PluginManifest {
  name: string;
  version: string;
  description?: string | null;
  commands?: string[];
  permissions?: string[];
}
interface Plugin {
  id: string; // = nombre en disco (identidad estable; disco = SoT)
  name: string;
  version: string;
  enabled: boolean;
  verified: boolean; // firma Ed25519 válida (fail-closed: false = no se ejecuta)
  manifest: PluginManifest;
  installed_at: string;
}
interface ToolResult {
  stdout: string;
  exit_ok: boolean;
  sandboxed_net_deny: boolean;
}
// spec-013 — un root del modelo Roots/readonly (superset estructurado de fs_read/fs_write).
interface FsRoot { path: string; readonly: boolean; }
interface Permissions {
  net: string[]; fs_read: string[]; fs_write: string[]; shell: boolean; secrets: string[];
  // audit-3 Codex: el backend declara fs_roots; omitirlo ocultaría roots al usuario.
  fs_roots?: FsRoot[];
}
interface SignedManifest {
  name: string; version: string; description?: string | null;
  entrypoint: string; permissions: Permissions;
}

/// Resumen legible de los permisos declarados en un manifiesto firmado (para que el usuario VEA
/// qué concede, incluido fs_roots). audit-3 Codex.
function permsSummary(perm: Permissions): string {
  const parts: string[] = [];
  parts.push(`net: ${perm.net.length ? perm.net.join(", ") : "ninguno"}`);
  if (perm.fs_read?.length) parts.push(`fs_read: ${perm.fs_read.join(", ")}`);
  if (perm.fs_write?.length) parts.push(`fs_write: ${perm.fs_write.join(", ")}`);
  if (perm.fs_roots?.length) parts.push(`roots: ${perm.fs_roots.map((r) => `${r.path}${r.readonly ? " (ro)" : " (rw)"}`).join(", ")}`);
  if (perm.shell) parts.push("shell: SÍ");
  if (perm.secrets?.length) parts.push(`secrets: ${perm.secrets.join(", ")}`);
  return parts.join(" · ");
}

// spec-003 — grant/revoke a plugin a named secret backed by an OS Keychain entry.
// The value never crosses here; only the keychain reference (service+account).
function SecretGrantPanel() {
  const [plugin, setPlugin] = useState("");
  const [secret, setSecret] = useState("");
  const [svc, setSvc] = useState("");
  const [acc, setAcc] = useState("");
  const [msg, setMsg] = useState<string | null>(null);
  const [err, setErr] = useState<string | null>(null);
  const grant = async () => {
    setMsg(null); setErr(null);
    try {
      await invoke("plugin_grant_secret", { name: plugin.trim(), secretName: secret.trim(), kcService: svc.trim(), kcAccount: acc.trim() });
      setMsg(`concedido ${secret} → keychain ${svc}/${acc} (valor nunca persistido)`);
    } catch (e) { setErr(String(e)); }
  };
  const revoke = async () => {
    setMsg(null); setErr(null);
    try { await invoke("plugin_revoke_secret", { name: plugin.trim(), secretName: secret.trim() }); setMsg(`revocado ${secret}`); }
    catch (e) { setErr(String(e)); }
  };
  return (
    <div className="card" style={{ marginBottom: 16 }}>
      <div style={{ fontWeight: 600, marginBottom: 8 }}>Secrets BYOK</div>
      <div style={{ display: "flex", gap: 8, flexWrap: "wrap", marginBottom: 8 }}>
        <input className="form-input" style={{ flex: "1 1 130px" }} placeholder="plugin" value={plugin} onChange={(e) => setPlugin(e.target.value)} />
        <input className="form-input" style={{ flex: "1 1 130px" }} placeholder="secret (declarado en manifest)" value={secret} onChange={(e) => setSecret(e.target.value)} />
        <input className="form-input" style={{ flex: "1 1 130px" }} placeholder="keychain service" value={svc} onChange={(e) => setSvc(e.target.value)} />
        <input className="form-input" style={{ flex: "1 1 100px" }} placeholder="keychain account" value={acc} onChange={(e) => setAcc(e.target.value)} />
        <Button variant="primary" size="sm" disabled={!plugin.trim() || !secret.trim() || !svc.trim()} onClick={grant}>Conceder</Button>
        <Button variant="secondary" size="sm" disabled={!plugin.trim() || !secret.trim()} onClick={revoke}>Revocar</Button>
      </div>
      <div className="muted" style={{ fontSize: 11 }}>Solo se concede si el manifest firmado declara ese secret. El valor se lee del Keychain en cada invoke; nunca se guarda ni loguea.</div>
      {msg && <div style={{ color: "var(--ok)", fontSize: 12, marginTop: 6, fontFamily: "var(--mono)" }}>{msg}</div>}
      {err && <div style={{ color: "var(--err)", fontSize: 12, marginTop: 6, fontFamily: "var(--mono)" }}>{err}</div>}
    </div>
  );
}

// spec-013 (T041) — marketplace catalog of the shipped, signed bundle plugins with
// tier + category. Install is one click → verify against the pinned key + harden.
interface BundlePluginInfo {
  name: string; version: string; description?: string | null;
  tier: string; category: string; is_mcp: boolean; verified: boolean;
  net: string[]; secrets: string[];
}

function BundleCatalogPanel({ onInstalled }: { onInstalled: () => void }) {
  const [items, setItems] = useState<BundlePluginInfo[]>([]);
  const [err, setErr] = useState<string | null>(null);
  const [msg, setMsg] = useState<string | null>(null);
  const load = useCallback(() => {
    invoke<BundlePluginInfo[]>("plugin_list_bundled")
      .then((xs) => { setItems(xs); setErr(null); })
      .catch((e) => setErr(String(e)));
  }, []);
  useEffect(() => { load(); }, [load]);

  const install = async (name: string) => {
    setMsg(null); setErr(null);
    try {
      const v = await invoke<string>("plugin_install_bundled", { name });
      setMsg(`instalado ${name} v${v} (firmado + read-only)`);
      onInstalled();
    } catch (e) { setErr(String(e)); }
  };

  if (items.length === 0) return null;
  // group by tier (already sorted tier-1, tier-2, first-party, tool by the backend).
  const tiers = [...new Set(items.map((i) => i.tier))];
  const tierLabel = (t: string) => ({
    "tier-1": "Tier 1 · mayor impacto", "tier-2": "Tier 2 · LSP / docs / git",
    "first-party": "First-party · propios", "tool": "Tools",
  } as Record<string, string>)[t] ?? t;

  return (
    <div className="card" style={{ marginBottom: 16 }}>
      <div style={{ fontWeight: 600, marginBottom: 8 }}>Bundle recomendado (MCP firmados)</div>
      <div className="muted" style={{ fontSize: 11, marginBottom: 10 }}>
        Plugins firmados (Ed25519) opt-in por agente. El binario real es runtime-dep: el launcher lo localiza y falla cerrado si falta. net default-deny · BYOK keychain. Los permisos de un MCP server son <b>declarados y auditados</b> (la firma los cubre); el sandbox de red/fs se aplica en el path por-tool, no al server que lanza el CLI del agente.
      </div>
      {tiers.map((t) => (
        <div key={t} style={{ marginBottom: 12 }}>
          <div className="muted" style={{ fontSize: 11, textTransform: "uppercase", letterSpacing: 0.5, marginBottom: 6 }}>{tierLabel(t)}</div>
          <div style={{ display: "flex", flexDirection: "column", gap: 6 }}>
            {items.filter((i) => i.tier === t).map((i) => (
              <div key={i.name} className="card" style={{ display: "flex", alignItems: "center", gap: 10, padding: 10 }}>
                <div style={{ flex: 1 }}>
                  <div style={{ fontWeight: 600 }}>
                    {pluginLabel(i.name)} <span className="muted" style={{ fontSize: 11 }}>v{i.version}</span>
                    {i.is_mcp && <span className="pill" style={{ fontSize: 9, marginLeft: 6 }}>MCP</span>}
                    <span className="pill" style={{ fontSize: 9, marginLeft: 4 }}>{i.category}</span>
                    {!i.verified && <span className="pill" style={{ fontSize: 9, marginLeft: 4, color: "var(--err)" }}>sin firma válida</span>}
                  </div>
                  {pluginDescription(i.description) && <div className="muted" style={{ fontSize: 12 }}>{pluginDescription(i.description)}</div>}
                  <div style={{ display: "flex", gap: 4, marginTop: 5, flexWrap: "wrap" }}>
                    {i.net.length === 0 && <span className="pill" style={{ fontSize: 9 }} title="declara cero hosts de red">net: ninguno</span>}
                    {i.net.map((h) => <span key={h} className="pill" style={{ fontSize: 9 }}>net: {h}</span>)}
                    {i.secrets.map((s) => <span key={s} className="pill" style={{ fontSize: 9 }}>BYOK: {s}</span>)}
                  </div>
                </div>
                <Button variant="secondary" size="sm" disabled={!i.verified} onClick={() => install(i.name)} title="Instalar (verifica firma + read-only)">Install</Button>
              </div>
            ))}
          </div>
        </div>
      ))}
      {msg && <div style={{ color: "var(--ok)", fontSize: 12, marginTop: 6, fontFamily: "var(--mono)" }}>{msg}</div>}
      {err && <div style={{ color: "var(--err)", fontSize: 12, marginTop: 6, fontFamily: "var(--mono)" }}>{err}</div>}
    </div>
  );
}

// spec-043 Ola 4 — Skills híbrido con verificación. Trust badges (verde/amarillo/rojo),
// discovery of importable local skills, import-through-the-gate, and "promover scripts".
interface SkillTrustRow {
  name: string;
  trust_level: string | null; // "verified"|"promoted"|"sandboxed"|"rejected"
  badge: string;
  inert: boolean;
  status_message: string | null;
  may_execute: boolean;
}
interface DiscoveredSkill {
  name: string;
  version: string;
  description: string;
  path: string;
  source: string;
  has_manifest: boolean;
}

// Badge color + label for a trust level. verde=firmado, amarillo=sandboxed/promoted,
// rojo=rechazado (igual que el verified=false fail-closed que Furx ya muestra).
function trustBadge(badge: string): { color: string; label: string; title: string } {
  switch (badge) {
    case "verified":
      return { color: "var(--ok)", label: "firmado", title: "Firma Furx válida + tree_hash verificado — scripts ejecutables" };
    case "promoted":
      return { color: "var(--amber)", label: "promovido localmente", title: "Vos confiaste en la fuente y firmaste el árbol localmente — scripts ejecutables (per-máquina)" };
    case "sandboxed":
      return { color: "var(--amber)", label: "trust-the-source · sandboxed", title: "Sin firma Furx — SKILL.md se usa como prompt; los scripts quedan inertes hasta que los promuevas" };
    case "rejected":
      return { color: "var(--err)", label: "rechazado", title: "Firma inválida o tree_hash no coincide — fail-closed, no ejecuta" };
    case "legacy":
      return { color: "var(--muted)", label: "plugin legacy", title: "Plugin clásico (entrypoint firmado), no un skill 043 — su confianza es la firma Ed25519 del manifest, sin tree_hash de skill" };
    default:
      return { color: "var(--muted)", label: badge, title: badge };
  }
}

function SkillsTrustPanel() {
  const [rows, setRows] = useState<SkillTrustRow[]>([]);
  const [revokeWarn, setRevokeWarn] = useState(false);
  const [discovered, setDiscovered] = useState<DiscoveredSkill[]>([]);
  const [err, setErr] = useState<string | null>(null);
  const [msg, setMsg] = useState<string | null>(null);
  const [importPath, setImportPath] = useState("");

  const reload = useCallback(() => {
    invoke<[SkillTrustRow[], boolean]>("skills_trust_list")
      .then(([rs, warn]) => { setRows(rs); setRevokeWarn(warn); setErr(null); })
      .catch((e) => setErr(String(e)));
  }, []);
  const discover = useCallback(() => {
    invoke<DiscoveredSkill[]>("skills_discover_local")
      .then((ds) => { setDiscovered(ds); setErr(null); })
      .catch((e) => setErr(String(e)));
  }, []);
  useEffect(() => { reload(); discover(); }, [reload, discover]);

  const importLocal = async (path: string) => {
    setMsg(null); setErr(null);
    try {
      const badge = await invoke<string>("skill_import_local", { path: path.trim() });
      setMsg(`importado: ${trustBadge(badge).label}`);
      reload();
    } catch (e) { setErr(String(e)); }
  };

  const promote = async (name: string) => {
    // Deliberate trust grant: confirm before making scripts executable.
    if (!window.confirm(`Promover los scripts de "${name}"?\n\nEsto los hace EJECUTABLES bajo el sandbox default-deny. Hacelo solo si confiás en la fuente de este skill.`)) return;
    setMsg(null); setErr(null);
    try {
      await invoke("skill_promote", { name });
      setMsg(`"${name}" promovido — scripts ejecutables`);
      reload();
    } catch (e) { setErr(String(e)); }
  };

  return (
    <div className="card" style={{ marginBottom: 16 }}>
      <div style={{ fontWeight: 600, marginBottom: 4 }}>Skills · verificación</div>
      <div className="muted" style={{ fontSize: 11, marginBottom: 10 }}>
        Furx verifica criptográficamente lo que baja. <span style={{ color: "var(--ok)" }}>Firmado</span> = ejecuta ·
        {" "}<span style={{ color: "var(--amber)" }}>trust-the-source</span> = SKILL.md como prompt, scripts inertes hasta promoverlos ·
        {" "}<span style={{ color: "var(--err)" }}>rechazado</span> = fail-closed.
      </div>

      {revokeWarn && (
        <div className="card" style={{ borderColor: "var(--err)", marginBottom: 10, padding: 8 }}>
          <span style={{ color: "var(--err)", fontSize: 12 }}>
            El archivo de revocación tiene líneas malformadas (se saltearon). Revisá <code>~/.furx/revoked_keys.txt</code> — cada línea debe ser un SHA-256 hex de 64 chars; las válidas sí se cargaron.
          </span>
        </div>
      )}

      {/* Installed skills with their trust badge. */}
      {rows.length > 0 && (
        <div style={{ display: "flex", flexDirection: "column", gap: 6, marginBottom: 12 }}>
          {rows.map((r) => {
            const b = trustBadge(r.badge);
            return (
              <div key={r.name} className="card" style={{ display: "flex", alignItems: "center", gap: 10, padding: 10 }}>
                <div style={{ flex: 1 }}>
                  <div style={{ fontWeight: 600 }}>
                    {pluginLabel(r.name)}
                    <span className="pill" style={{ fontSize: 9, marginLeft: 8, color: b.color, borderColor: b.color }} title={b.title}>{b.label}</span>
                    {r.inert && <span className="pill" style={{ fontSize: 9, marginLeft: 4 }} title="los scripts no ejecutan">scripts inertes</span>}
                  </div>
                  {r.status_message && <div className="muted" style={{ fontSize: 11, fontFamily: "var(--mono)" }}>{r.status_message}</div>}
                </div>
                {r.trust_level === "sandboxed" && !r.may_execute && (
                  <Button variant="secondary" size="sm" onClick={() => promote(r.name)} title="Hacer ejecutables los scripts (confiás en la fuente)">Promover scripts</Button>
                )}
              </div>
            );
          })}
        </div>
      )}

      {/* Import a local skill through the gate. */}
      <div style={{ display: "flex", gap: 8, flexWrap: "wrap", marginBottom: 10 }}>
        <input className="form-input" style={{ flex: "1 1 220px", fontFamily: "var(--mono)" }} placeholder="ruta local del skill (con SKILL.md)" value={importPath} onChange={(e) => setImportPath(e.target.value)} />
        <Button variant="primary" size="sm" disabled={!importPath.trim()} onClick={() => importLocal(importPath)} title="Importa pasando por el gate (firma + tree_hash + install-only)">Importar</Button>
        <Button variant="secondary" size="sm" onClick={discover} title="Re-escanear sources.user.toml">Descubrir</Button>
      </div>

      {/* Discovered (not yet installed) local skills from sources.user.toml. */}
      {discovered.length > 0 && (
        <div style={{ marginTop: 4 }}>
          <div className="muted" style={{ fontSize: 11, marginBottom: 6 }}>Descubiertos (Hermes/OpenClaw · <code>~/.furx/sources.user.toml</code>)</div>
          <div style={{ display: "flex", flexDirection: "column", gap: 6 }}>
            {discovered.map((d) => (
              <div key={`${d.source}:${d.name}`} className="card" style={{ display: "flex", alignItems: "center", gap: 10, padding: 8 }}>
                <div style={{ flex: 1 }}>
                  <div style={{ fontWeight: 600, fontSize: 13 }}>
                    {pluginLabel(d.name)} <span className="muted" style={{ fontSize: 11 }}>v{d.version}</span>
                    <span className="pill" style={{ fontSize: 9, marginLeft: 6 }}>{d.source}</span>
                    {d.has_manifest
                      ? <span className="pill" style={{ fontSize: 9, marginLeft: 4, color: "var(--ok)" }} title="trae manifest.json firmado — el gate decide la confianza">candidato firmado</span>
                      : <span className="pill" style={{ fontSize: 9, marginLeft: 4, color: "var(--amber)" }} title="sin manifest — importará como sandboxed">sin firma</span>}
                  </div>
                  {d.description && <div className="muted" style={{ fontSize: 11 }}>{d.description}</div>}
                </div>
                <Button variant="secondary" size="sm" onClick={() => importLocal(d.path)} title="Importar pasando por el gate">Importar</Button>
              </div>
            ))}
          </div>
        </div>
      )}

      {msg && <div style={{ color: "var(--ok)", fontSize: 12, marginTop: 8, fontFamily: "var(--mono)" }}>{msg}</div>}
      {err && <div style={{ color: "var(--err)", fontSize: 12, marginTop: 8, fontFamily: "var(--mono)" }}>{err}</div>}
    </div>
  );
}

export function PluginsView() {
  const [plugins, setPlugins] = useState<Plugin[]>([]);
  const [err, setErr] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  // invoke panel
  const [pName, setPName] = useState("");
  const [pTool, setPTool] = useState("list");
  const [pArgs, setPArgs] = useState("{}");
  const [result, setResult] = useState<ToolResult | null>(null);
  const [invErr, setInvErr] = useState<string | null>(null);
  const [running, setRunning] = useState(false);
  // ask-on-first-use consent prompt
  const [grantReq, setGrantReq] = useState<{ name: string; version: string; perms: SignedManifest["permissions"] } | null>(null);
  // plugin action feedback (verify / permisos / harden)
  const [actionMsg, setActionMsg] = useState<{ id: string; text: string; ok: boolean } | null>(null);

  const reload = useCallback(() => {
    setLoading(true);
    invoke<Plugin[]>("plugins_list")
      .then((ps) => { setPlugins(ps); setErr(null); })
      .catch((e) => setErr(String(e)))
      .finally(() => setLoading(false));
  }, []);

  // Toolbar: rescan ~/.furx/plugins/ and refresh list.
  const rescan = async () => {
    try {
      await invoke<PluginManifest[]>("plugins_scan");
      reload();
    } catch (e) { setErr(String(e)); }
  };

  // Per-plugin: verify Ed25519 signature.
  // `plugin_verify` espera un SignedManifest completo (entrypoint + permissions con
  // otro shape), no el PluginManifest normalizado de la lista. Pasar `p.manifest`
  // hacía fallar la deserialización de Tauri. Flujo correcto: leer el SignedManifest
  // firmado desde disco con `plugin_manifest({name})` (que ya verifica la firma y
  // devuelve Err si es inválida) y recién entonces `plugin_verify({manifest})`.
  const verify = async (p: Plugin) => {
    setActionMsg(null);
    try {
      const manifest = await invoke<SignedManifest>("plugin_manifest", { name: p.name });
      const ok = await invoke<boolean>("plugin_verify", { manifest });
      // Mostrar TODOS los permisos declarados (incl. fs_roots) junto con el resultado de la firma,
      // para que verificar sea informativo y no oculte roots (audit-3 Codex).
      const summary = ok ? ` · permisos: ${permsSummary(manifest.permissions)}` : "";
      setActionMsg({ id: p.id, text: (ok ? "firma válida ✓" : "firma inválida ✗") + summary, ok });
    } catch (e) {
      // plugin_manifest devuelve "signature invalid" si la firma no valida (fail-closed).
      setActionMsg({ id: p.id, text: String(e), ok: false });
    }
  };

  // Per-plugin: check if user has granted this plugin.
  const checkPerms = async (p: Plugin) => {
    setActionMsg(null);
    try {
      const granted = await invoke<boolean>("plugin_is_granted", { name: p.name, version: p.version });
      setActionMsg({ id: p.id, text: granted ? "acceso concedido ✓" : "no concedido (requires plugin_grant)", ok: granted });
    } catch (e) { setActionMsg({ id: p.id, text: String(e), ok: false }); }
  };

  // Per-plugin: harden dir to read-only.
  const harden = async (p: Plugin) => {
    setActionMsg(null);
    try {
      await invoke<void>("plugin_harden", { name: p.name });
      setActionMsg({ id: p.id, text: "hardened (read-only) ✓", ok: true });
    } catch (e) { setActionMsg({ id: p.id, text: String(e), ok: false }); }
  };

  useEffect(() => { reload(); }, [reload]);

  const toggle = async (p: Plugin) => {
    await invoke("plugins_set_enabled", { id: p.id, enabled: !p.enabled }).catch((e) => setErr(String(e)));
    reload();
  };

  const runTool = async () => {
    setRunning(true); setInvErr(null); setResult(null); setGrantReq(null);
    try {
      const r = await invoke<ToolResult>("plugin_invoke", { name: pName.trim(), tool: pTool.trim(), argsJson: pArgs });
      setResult(r);
    } catch (e) {
      const msg = String(e);
      // Ask-on-first-use: surface the requested permissions for consent.
      const m = msg.match(/NEEDS_GRANT:([^:]+):(.+)$/);
      if (m) {
        try {
          const man = await invoke<SignedManifest>("plugin_manifest", { name: m[1] });
          setGrantReq({ name: m[1], version: m[2], perms: man.permissions });
        } catch (e2) { setInvErr(String(e2)); }
      } else {
        setInvErr(msg);
      }
    } finally { setRunning(false); }
  };

  const [installMsg, setInstallMsg] = useState<string | null>(null);
  const installBundled = async () => {
    const name = pName.trim();
    if (!name) return;
    setInstallMsg(null); setInvErr(null);
    try {
      const v = await invoke<string>("plugin_install_bundled", { name });
      setInstallMsg(`instalado ${name} v${v} (firmado + read-only)`);
      reload();
    } catch (e) { setInvErr(String(e)); }
  };

  const grantAndRun = async () => {
    if (!grantReq) return;
    // version is taken from the verified on-disk manifest by the host, not sent here.
    await invoke("plugin_grant", { name: grantReq.name }).catch((e) => setInvErr(String(e)));
    setGrantReq(null);
    runTool();
  };

  return (
    <div className="page">
      <div className="page-header">
        <div className="page-title">Plugins</div>
        <div className="page-sub">extensiones globales · firma Ed25519 · permisos default-deny · sandbox net</div>
        <Button variant="secondary" size="sm" onClick={rescan} title="Rescanear ~/.furx/plugins/ y refrescar la lista">Reescanear</Button>
      </div>

      {loading && <div className="muted" style={{ padding: 16 }}>cargando…</div>}
      {err && <div className="empty"><div className="body" style={{ color: "var(--err)" }}>{err}</div></div>}

      {!loading && plugins.length === 0 && (
        <div className="empty">
          <span className="glyph-sm" aria-hidden="true" />
          <div className="head">Sin plugins instalados</div>
          <div className="body">Instalá un plugin firmado en <code>~/.furx/plugins/&lt;name&gt;/</code>, o usá el bundle recomendado de abajo.</div>
        </div>
      )}

      {plugins.length > 0 && (
        <div style={{ display: "flex", flexDirection: "column", gap: 8, marginBottom: 24 }}>
          <div className="muted" style={{ fontSize: 11, marginBottom: 2 }}>Instalados en <code>~/.furx/plugins/</code> ({plugins.length})</div>
          {plugins.map((p) => (
            <div
              key={p.id}
              className="card"
              style={{ display: "flex", alignItems: "center", gap: 12, opacity: p.enabled ? 1 : 0.6 }}
            >
              <div style={{ flex: 1 }}>
                <div style={{ fontWeight: 600 }}>
                  {pluginLabel(p.name)} <span className="muted" style={{ fontSize: 11 }}>v{p.version}</span>
                  {!p.verified && (
                    <span className="pill" style={{ fontSize: 9, marginLeft: 6, color: "var(--err)" }} title="firma Ed25519 ausente/inválida — el motor rehúsa ejecutarlo">sin firma válida</span>
                  )}
                  {!p.enabled && (
                    <span className="pill" style={{ fontSize: 9, marginLeft: 6 }}>desactivado</span>
                  )}
                </div>
                {pluginDescription(p.manifest.description) && <div className="muted" style={{ fontSize: 12 }}>{pluginDescription(p.manifest.description)}</div>}
                <div style={{ display: "flex", gap: 6, marginTop: 6, flexWrap: "wrap" }}>
                  {(p.manifest.permissions ?? []).map((perm) => (
                    <span key={perm} className="pill" style={{ fontSize: 10 }}>{perm}</span>
                  ))}
                </div>
              </div>
              <div style={{ display: "flex", flexDirection: "column", gap: 4, alignItems: "flex-end" }}>
                <Button variant="secondary" size="sm" onClick={() => toggle(p)}>
                  {p.enabled ? "Disable" : "Enable"}
                </Button>
                <div style={{ display: "flex", gap: 4 }}>
                  <Button variant="ghost" size="sm" onClick={() => verify(p)} title="Verificar firma Ed25519">Verificar</Button>
                  <Button variant="ghost" size="sm" onClick={() => checkPerms(p)} title="Ver si el usuario concedió acceso a este plugin">Permisos</Button>
                  <Button variant="ghost" size="sm" onClick={() => harden(p)} title="Hacer read-only el directorio del plugin">Hardear</Button>
                </div>
                {actionMsg && actionMsg.id === p.id && (
                  <div style={{ fontSize: 11, fontFamily: "var(--mono)", color: actionMsg.ok ? "var(--ok)" : "var(--err)" }}>
                    {actionMsg.text}
                  </div>
                )}
              </div>
            </div>
          ))}
        </div>
      )}

      {/* spec-043 Ola 4 — Skills híbrido con verificación: badges + import + promover. */}
      <SkillsTrustPanel />

      {/* spec-013 marketplace — install signed bundle MCP plugins by tier. */}
      <BundleCatalogPanel onInstalled={reload} />

      {/* Secrets BYOK panel — grant a plugin a named secret from the OS Keychain. */}
      <SecretGrantPanel />

      {/* "Instalar desde manifiesto" eliminado: `plugins_install` solo insertaba una
          fila en la tabla `plugins`, pero `plugins_list` usa el DISCO como fuente de
          verdad (`~/.furx/plugins/<name>/`). Un manifest pegado nunca creaba un plugin
          usable — desaparecía al recargar. La instalación real pasa por el bundle
          firmado (`BundleCatalogPanel` / `plugin_install_bundled`), que escribe a disco
          con la firma verificada. No reintroducir UI de install-from-JSON sin un
          backend que materialice el plugin en disco. */}

      {/* Invoke panel — runs a plugin tool through the signature-verified host. */}
      <div className="card">
        <div style={{ fontWeight: 600, marginBottom: 10 }}>Invocar herramienta</div>
        <div style={{ display: "flex", gap: 8, flexWrap: "wrap", marginBottom: 8 }}>
          <input className="form-input" style={{ flex: "1 1 160px" }} placeholder="plugin (ej. filesystem-ls)" value={pName} onChange={(e) => setPName(e.target.value)} />
          <input className="form-input" style={{ flex: "1 1 120px" }} placeholder="tool" value={pTool} onChange={(e) => setPTool(e.target.value)} />
          <input className="form-input" style={{ flex: "1 1 160px", fontFamily: "var(--mono)" }} placeholder='{"path":"."}' value={pArgs} onChange={(e) => setPArgs(e.target.value)} />
          <Button variant="secondary" size="sm" disabled={!pName.trim()} onClick={installBundled} title="Instalar desde el bundle firmado">Install</Button>
          <Button variant="primary" size="sm" disabled={running || !pName.trim()} onClick={runTool}>{running ? "…" : "Run"}</Button>
        </div>
        {installMsg && <div style={{ color: "var(--ok)", fontSize: 12, marginTop: 6, fontFamily: "var(--mono)" }}>{installMsg}</div>}
        <div className="muted" style={{ fontSize: 11 }}>El host verifica la firma del manifest y deniega red/secrets sin grant explícito (BYOK).</div>
        {grantReq && (
          <div style={{ marginTop: 10, padding: 12, border: "1px solid var(--cyan-dim)", borderRadius: 8, background: "var(--cyan-glow)" }}>
            <div style={{ fontWeight: 600, marginBottom: 6 }}>{grantReq.name} v{grantReq.version} pide permisos</div>
            <div style={{ display: "flex", gap: 6, flexWrap: "wrap", marginBottom: 8 }}>
              {grantReq.perms.net.length > 0 && <span className="pill">net: {grantReq.perms.net.join(", ")}</span>}
              {grantReq.perms.fs_read.length > 0 && <span className="pill">fs-read: {grantReq.perms.fs_read.join(", ")}</span>}
              {grantReq.perms.fs_write.length > 0 && <span className="pill">fs-write: {grantReq.perms.fs_write.join(", ")}</span>}
              {grantReq.perms.shell && <span className="pill">shell</span>}
              {grantReq.perms.secrets.length > 0 && <span className="pill" style={{ color: "var(--amber)" }}>secrets: {grantReq.perms.secrets.join(", ")}</span>}
              {grantReq.perms.net.length === 0 && grantReq.perms.fs_write.length === 0 && !grantReq.perms.shell && grantReq.perms.secrets.length === 0 && <span className="pill">solo lectura · sin red</span>}
            </div>
            <Button variant="primary" size="sm" onClick={grantAndRun}>Conceder y ejecutar</Button>
            <Button variant="secondary" size="sm" style={{ marginLeft: 8 }} onClick={() => setGrantReq(null)}>Cancelar</Button>
          </div>
        )}
        {invErr && <div style={{ color: "var(--err)", fontSize: 12, marginTop: 8, fontFamily: "var(--mono)" }}>{invErr}</div>}
        {result && (
          <pre style={{ marginTop: 10, background: "#16130f", color: "#e8e8e3", padding: 12, borderRadius: 8, fontSize: 12, whiteSpace: "pre-wrap", overflowX: "auto" }}>
            <div className="muted" style={{ fontSize: 10, marginBottom: 6 }}>
              exit_ok={String(result.exit_ok)} · net_sandboxed={String(result.sandboxed_net_deny)}
            </div>
            {result.stdout}
          </pre>
        )}
      </div>
    </div>
  );
}
