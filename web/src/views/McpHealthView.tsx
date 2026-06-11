import { useEffect, useState, useCallback } from "react";
import { invoke as rawInvoke } from "@tauri-apps/api/core";
// 045 FR-002 — mcp_set_enabled requiere confirmación (gated): usar el invoke con flujo de aprobación.
import { invoke } from "../lib/invoke";
import { McpHealthReport, DiscoveredMcp } from "../types";
import { Button } from "../components/Button";

// McpOverride: { name, enabled, source } — ver mcp_overrides_list command.
interface McpOverrideRow {
  name: string;
  enabled: boolean;
  source: string; // "user" | "discovery"
}

export function McpHealthView() {
  const [report, setReport] = useState<McpHealthReport | null>(null);
  const [discovered, setDiscovered] = useState<DiscoveredMcp[]>([]);
  const [loading, setLoading] = useState(false);
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState<string | null>(null);
  const [overrides, setOverrides] = useState<McpOverrideRow[]>([]);
  const [overridesErr, setOverridesErr] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    setLoading(true);
    try { const r = await rawInvoke<McpHealthReport>("mcp_health"); setReport(r); }
    finally { setLoading(false); }
  }, []);

  const discover = useCallback(async () => {
    try { setDiscovered(await rawInvoke<DiscoveredMcp[]>("mcp_discover")); }
    catch (e) { setErr(String(e)); }
  }, []);

  const loadOverrides = useCallback(async () => {
    try {
      // mcp_overrides_list returns Vec<McpOverride> = [{name, enabled, source}]
      const rows = await rawInvoke<McpOverrideRow[]>("mcp_overrides_list");
      setOverrides(rows);
      setOverridesErr(null);
    } catch (e) { setOverridesErr(String(e)); }
  }, []);

  useEffect(() => { refresh(); discover(); loadOverrides(); const id = setInterval(refresh, 30000); return () => clearInterval(id); }, [refresh, discover, loadOverrides]);

  const toggle = async (name: string, next: boolean) => {
    setErr(null); setBusy(true);
    try { await invoke("mcp_set_enabled", { name, enabled: next }); await refresh(); }
    catch (e) { setErr(String(e)); }
    finally { setBusy(false); }
  };

  const upCount = report ? report.servers.filter((s) => s.healthy && s.enabled).length : 0;
  const newDiscovered = discovered.filter((d) => !d.already_configured);

  return (
    <div className="page">
      <div className="page-header">
        <div className="page-title">MCP server health</div>
        <div className="page-sub">{report?.config_path ?? "no .claude.json found"} · {report ? `${upCount}/${report.servers.length} activos` : "—"}</div>
      </div>
      <Button variant="ghost" onClick={refresh} disabled={loading || busy}>{loading ? "verificando…" : "Refresh"}</Button>
      {err && <div className="muted" role="alert" style={{ color: "var(--danger, #d33)", marginTop: 8 }}>{err}</div>}
      {report && (
        <div className="mon-grid" style={{ marginTop: 14 }}>
          {report.servers.map((s) => {
            const toolsTip = s.tools_sample && s.tools_sample.length > 0
              ? `tools: ${s.tools_sample.join(", ")}${(s.tools_count ?? 0) > s.tools_sample.length ? `, … (${s.tools_count} total)` : ""}`
              : (s.tools_count != null ? `${s.tools_count} tools` : "tools/list not available for this server");
            return (
              <div key={s.name} className={`mon ${!s.enabled ? "" : (s.healthy ? "up" : "down")}`} title={toolsTip} style={!s.enabled ? { opacity: 0.55 } : undefined}>
                <div className="mon-head">
                  <span className={`dot ${!s.enabled ? "unknown" : (s.healthy ? "up" : "down")}`} />
                  <span className="mon-label">{s.name}</span>
                  <span className="mon-addr muted">{s.transport}</span>
                  {s.tools_count != null && (
                    <span className="sev-tag sev-info" style={{ marginLeft: 8, fontVariantNumeric: "tabular-nums" }} aria-label={`${s.tools_count} tools`}>
                      {s.tools_count} tools
                    </span>
                  )}
                  {/* 045 FR-002 — toggle enabled/disabled (DB override, NO toca ~/.claude.json). */}
                  <button
                    type="button"
                    className={s.enabled ? "ghost is-active" : "ghost"}
                    aria-pressed={s.enabled}
                    aria-label={`${s.enabled ? "Desactivar" : "Activar"} ${s.name}`}
                    title={s.enabled ? "Desactivar para Furx (no edita ~/.claude.json)" : "Activar para Furx"}
                    disabled={busy}
                    onClick={() => toggle(s.name, !s.enabled)}
                    style={{ marginLeft: s.tools_count != null ? 8 : "auto" }}
                  >
                    {s.enabled ? "On" : "Off"}
                  </button>
                </div>
                <div className="mon-body">
                  {!s.enabled
                    ? <span className="muted">deshabilitado por vos</span>
                    : s.healthy ? <>ok · {s.latency_ms ?? "?"}ms</> : <span className="muted">{s.error ?? "down"}</span>}
                </div>
              </div>
            );
          })}
        </div>
      )}

      {/* 045 FR-002 — auto-discovery: binarios mcp-* del $PATH como SUGERENCIA (no auto-instala). */}
      <div className="page-header" style={{ marginTop: 20 }}>
        <div className="page-title" style={{ fontSize: "1rem" }}>Descubiertos en PATH</div>
        <div className="page-sub">
          {newDiscovered.length === 0
            ? "ningún binario mcp-* nuevo en tu PATH"
            : `${newDiscovered.length} binario(s) mcp-* no configurado(s) — agregalos a ~/.claude.json para usarlos`}
        </div>
      </div>
      {newDiscovered.length > 0 && (
        <ul className="discover-list" style={{ marginTop: 6, listStyle: "none", padding: 0 }}>
          {newDiscovered.map((d) => (
            <li key={d.path} className="muted" style={{ display: "flex", gap: 8, alignItems: "baseline", padding: "2px 0" }}>
              <code>{d.binary}</code>
              <span className="muted" style={{ fontSize: "0.85em" }}>{d.path}</span>
            </li>
          ))}
        </ul>
      )}

      {/* 053 — Overrides: persistidos en DB via mcp_overrides_list */}
      <div className="page-header" style={{ marginTop: 20 }}>
        <div className="page-title" style={{ fontSize: "1rem" }}>Overrides</div>
        <div className="page-sub">Estado persistido de enables/disables por Furx (DB, no toca ~/.claude.json)</div>
        <Button variant="ghost" size="sm" onClick={loadOverrides}>Refrescar</Button>
      </div>
      {overridesErr && <div className="muted" role="alert" style={{ color: "var(--danger, #d33)", marginTop: 8 }}>{overridesErr}</div>}
      {overrides.length === 0 && !overridesErr && (
        <div className="muted" style={{ marginTop: 6, fontSize: 12 }}>Sin overrides persistidos.</div>
      )}
      {overrides.length > 0 && (
        <table style={{ width: "100%", fontSize: 12, marginTop: 8, borderCollapse: "collapse" }}>
          <thead>
            <tr>
              <th style={{ textAlign: "left", padding: "4px 8px" }}>Servidor</th>
              <th style={{ textAlign: "left", padding: "4px 8px" }}>Estado</th>
              <th style={{ textAlign: "left", padding: "4px 8px" }}>Fuente</th>
            </tr>
          </thead>
          <tbody>
            {overrides.map((o) => (
              <tr key={o.name} className="audit-row" style={{ display: "table-row" }}>
                <td style={{ fontFamily: "var(--mono)", padding: "4px 8px" }}>{o.name}</td>
                <td style={{ padding: "4px 8px" }}>
                  <span className={`sev-tag sev-${o.enabled ? "info" : "warning"}`}>{o.enabled ? "habilitado" : "deshabilitado"}</span>
                </td>
                <td className="muted" style={{ padding: "4px 8px" }}>{o.source}</td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
    </div>
  );
}
