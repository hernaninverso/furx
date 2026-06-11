// web/src/views/PresetView.tsx — 053 UI para gestión de preset overrides (preset_overrides_list / preset_override_set).
//
// Backend: preset_overrides_list() → PresetOverride[], preset_override_set(preset, provider_alias, enabled) → void.
// Un override permite forzar habilitado/deshabilitado a un provider_alias dentro de un preset de AIE.

import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

interface PresetOverride {
  preset: string;
  provider_alias: string;
  enabled: boolean;
  updated_at: string;
}

const KNOWN_PRESETS = ["frontier_free", "bulk_free", "fast_small_free", "internal_dev"] as const;

export function PresetView() {
  const [overrides, setOverrides] = useState<PresetOverride[]>([]);
  const [msg, setMsg] = useState<string | null>(null);
  const [err, setErr] = useState<string | null>(null);
  const [formPreset, setFormPreset] = useState<string>(KNOWN_PRESETS[0]);
  // 053 fix: cuando se elige "otro" (formPreset === ""), el preset real se escribe acá.
  // Sin esto, "otro" guardaba un preset vacío.
  const [customPreset, setCustomPreset] = useState("");
  const [formAlias, setFormAlias] = useState("");
  const [formEnabled, setFormEnabled] = useState(true);
  const [busy, setBusy] = useState(false);

  const refresh = async () => {
    try {
      setOverrides(await invoke<PresetOverride[]>("preset_overrides_list"));
    } catch (e) {
      setErr(String(e));
    }
  };

  useEffect(() => { void refresh(); }, []);

  // El preset efectivo: el del select, o el texto libre cuando se eligió "otro".
  const effectivePreset = formPreset === "" ? customPreset.trim() : formPreset;

  const submit = async () => {
    if (!effectivePreset) { setErr("El preset es obligatorio."); return; }
    if (!formAlias.trim()) { setErr("provider_alias es obligatorio."); return; }
    setBusy(true); setErr(null); setMsg(null);
    try {
      await invoke("preset_override_set", {
        preset: effectivePreset,
        providerAlias: formAlias.trim(),
        enabled: formEnabled,
      });
      setMsg(`Override guardado: ${effectivePreset} / ${formAlias.trim()} → ${formEnabled ? "habilitado" : "deshabilitado"}.`);
      setFormAlias("");
      await refresh();
    } catch (e) {
      setErr(String(e));
    } finally {
      setBusy(false);
    }
  };

  // Agrupar por preset para lectura más clara
  const byPreset: Record<string, PresetOverride[]> = {};
  for (const o of overrides) {
    if (!byPreset[o.preset]) byPreset[o.preset] = [];
    byPreset[o.preset].push(o);
  }

  return (
    <div className="page preset-view">
      <div className="page-header">
        <div className="page-title">Preset overrides</div>
        <div className="page-sub">
          Habilitar / deshabilitar providers por preset en AIE. {overrides.length} override(s) activo(s).
        </div>
      </div>

      {msg && <div className="toast-inline">{msg}</div>}
      {err && <div className="toast-inline" style={{ borderColor: "var(--danger, #d33)", color: "var(--danger, #d33)" }}>{err}</div>}

      {/* Lista de overrides agrupada por preset */}
      {overrides.length === 0 ? (
        <div className="empty" style={{ marginBottom: 16 }}>
          <div className="head">Sin overrides</div>
          <div className="body muted">Los presets usan su configuración default.</div>
        </div>
      ) : (
        <div style={{ marginBottom: 16 }}>
          {Object.entries(byPreset).map(([preset, items]) => (
            <div key={preset} style={{ marginBottom: 12, padding: "10px 14px", background: "var(--surface)", borderRadius: 8, border: "1px solid var(--border)" }}>
              <div style={{ fontSize: 13, fontWeight: 600, marginBottom: 8, fontFamily: "var(--mono)" }}>{preset}</div>
              <table style={{ width: "100%", fontSize: 12, borderCollapse: "collapse" }}>
                <thead>
                  <tr style={{ textAlign: "left", borderBottom: "1px solid var(--border)" }}>
                    <th style={{ padding: "4px 8px" }}>provider_alias</th>
                    <th style={{ padding: "4px 8px" }}>Estado</th>
                    <th style={{ padding: "4px 8px" }}>Actualizado</th>
                    <th style={{ padding: "4px 8px" }}>Acción</th>
                  </tr>
                </thead>
                <tbody>
                  {items.map((o) => (
                    <tr key={`${o.preset}/${o.provider_alias}`} style={{ borderBottom: "1px solid var(--border)" }}>
                      <td style={{ padding: "4px 8px", fontFamily: "var(--mono)" }}>{o.provider_alias}</td>
                      <td style={{ padding: "4px 8px" }}>
                        <span className={`sev-tag ${o.enabled ? "sev-info" : ""}`}>{o.enabled ? "habilitado" : "deshabilitado"}</span>
                      </td>
                      <td style={{ padding: "4px 8px", fontSize: 11, fontFamily: "var(--mono)" }}>{o.updated_at.slice(0, 16)}</td>
                      <td style={{ padding: "4px 8px" }}>
                        <button
                          className="fxc-btn"
                          style={{ fontSize: 11, padding: "2px 8px" }}
                          disabled={busy}
                          onClick={async () => {
                            setBusy(true); setErr(null); setMsg(null);
                            try {
                              await invoke("preset_override_set", {
                                preset: o.preset,
                                providerAlias: o.provider_alias,
                                enabled: !o.enabled,
                              });
                              setMsg(`${o.provider_alias} → ${!o.enabled ? "habilitado" : "deshabilitado"}.`);
                              await refresh();
                            } catch (e) { setErr(String(e)); }
                            finally { setBusy(false); }
                          }}
                        >
                          {o.enabled ? "Deshabilitar" : "Habilitar"}
                        </button>
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          ))}
        </div>
      )}

      {/* Formulario agregar override */}
      <div style={{ padding: "12px 14px", background: "var(--surface)", borderRadius: 8, border: "1px solid var(--border)" }}>
        <div style={{ fontSize: 13, fontWeight: 600, marginBottom: 10 }}>Agregar override</div>
        <div style={{ display: "grid", gridTemplateColumns: "1fr 1fr 1fr auto", gap: 8, alignItems: "end" }}>
          <label style={{ fontSize: 12 }}>
            Preset
            <select
              value={formPreset}
              onChange={(e) => setFormPreset(e.target.value)}
              style={{ display: "block", width: "100%", marginTop: 4, padding: "5px 8px", borderRadius: 5, border: "1px solid var(--border)", background: "var(--bg, #0e0e0e)", color: "var(--text)", fontSize: 12, boxSizing: "border-box" }}
            >
              {KNOWN_PRESETS.map((p) => <option key={p} value={p}>{p}</option>)}
              <option value="">otro (escribir abajo)</option>
            </select>
            {formPreset === "" && (
              <input
                value={customPreset}
                onChange={(e) => setCustomPreset(e.target.value)}
                placeholder="nombre del preset"
                style={{ display: "block", width: "100%", marginTop: 6, padding: "5px 8px", borderRadius: 5, border: "1px solid var(--border)", background: "var(--bg, #0e0e0e)", color: "var(--text)", fontSize: 12, boxSizing: "border-box" }}
              />
            )}
          </label>
          <label style={{ fontSize: 12 }}>
            provider_alias *
            <input
              value={formAlias}
              onChange={(e) => setFormAlias(e.target.value)}
              placeholder="ej: cerebras, groq, mistral"
              style={{ display: "block", width: "100%", marginTop: 4, padding: "5px 8px", borderRadius: 5, border: "1px solid var(--border)", background: "var(--bg, #0e0e0e)", color: "var(--text)", fontSize: 12, boxSizing: "border-box" }}
            />
          </label>
          <label style={{ fontSize: 12 }}>
            Estado
            <select
              value={formEnabled ? "1" : "0"}
              onChange={(e) => setFormEnabled(e.target.value === "1")}
              style={{ display: "block", width: "100%", marginTop: 4, padding: "5px 8px", borderRadius: 5, border: "1px solid var(--border)", background: "var(--bg, #0e0e0e)", color: "var(--text)", fontSize: 12, boxSizing: "border-box" }}
            >
              <option value="1">Habilitado</option>
              <option value="0">Deshabilitado</option>
            </select>
          </label>
          <button
            className="fxc-btn"
            onClick={() => void submit()}
            disabled={busy || !formAlias.trim() || !effectivePreset}
            style={{ height: 30 }}
          >
            {busy ? "…" : "Guardar"}
          </button>
        </div>
        <p className="muted" style={{ fontSize: 11, marginTop: 8, marginBottom: 0 }}>
          Los overrides persisten en DB local. Un upsert por (preset, provider_alias): guardar dos veces actualiza el estado.
        </p>
      </div>
    </div>
  );
}
