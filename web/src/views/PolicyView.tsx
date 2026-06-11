// web/src/views/PolicyView.tsx — 053 UI para gestión de reglas custom de policy (027 F2).
//
// Backend: policy_list_rules → StoredRule[] (decision/risk como STRINGS snake_case),
// policy_set_rule(rule: CustomRule) → void  [CustomRule.decision = Decision TAGGED {kind}],
// policy_remove_rule(id) → void, policy_preview(commandId, agentProfile?, plugin?) → PolicyPreview
// (default/effective_decision = Decision TAGGED {kind}), policy_set_custom_enabled(enabled) → void.
//
// invoke desde ../lib/invoke: policy_set_rule / policy_remove_rule / policy_set_custom_enabled pasan
// por el gate de aprobación universal (requires_confirmation) — el wrapper encola el pedido y lo
// resuelve el GlobalApprovalModal. El `invoke` crudo NO abre ese flujo (audit-3 Codex).

import { useEffect, useState } from "react";
import { invoke } from "../lib/invoke";

interface StoredRule {
  id: string;
  description: string;
  match_command: string | null;
  match_risk: string | null;
  match_agent_profile: string | null;
  match_plugin: string | null;
  decision: string; // snake_case: "deny" | "require_approval" | "require_n_approvals:N"
  enabled: boolean;
}

// Decision serializa TAGGED en Rust: `{ kind: "require_approval" }` / `{ kind: "deny" }`.
type Decision = { kind: string; n?: number };

interface CustomRule {
  id: string;
  description: string;
  match_command: string | null;
  match_risk: string | null; // snake_case del enum Risk
  match_agent_profile: string | null;
  match_plugin: string | null;
  decision: Decision;
}

interface PolicyPreview {
  default_decision: Decision;
  effective_decision: Decision;
  applied_rule: unknown;
  hardened_by_custom: boolean;
  custom_enabled: boolean;
}

// Decisión: el value es el `kind` que el backend espera dentro de `{ kind }`. Hardening-only ⇒ solo
// require_approval / deny (Allow relajaría; el backend lo rechaza).
const DECISION_OPTIONS = [
  { kind: "require_approval", label: "Requiere aprobación" },
  { kind: "require_n_approvals", label: "Requiere N aprobaciones" },
  { kind: "deny", label: "Denegar" },
] as const;

// Risk: los valores REALES del enum Rust (snake_case), no Low/Medium/High.
const RISK_OPTIONS = [
  { value: "safe", label: "Safe" },
  { value: "destructive", label: "Destructive" },
  { value: "credential", label: "Credential" },
  { value: "external", label: "External" },
] as const;

/// Extrae el texto legible de una Decision (tagged {kind}) o de un string crudo de la DB.
function decisionLabel(d: Decision | string | null | undefined): string {
  if (d == null) return "—";
  const kind = typeof d === "string" ? d.split(":")[0] : d.kind;
  switch (kind) {
    case "deny": return "Denegar";
    case "require_approval": return "Requiere aprobación";
    case "require_n_approvals": return "Requiere N aprobaciones";
    case "allow": return "Permitir";
    default: return kind;
  }
}

/// Normaliza el `decision` string de una StoredRule al `kind` del formulario. PRESERVA
/// `require_n_approvals` (audit-3 Codex: degradarlo a require_approval reducía silenciosamente el N).
function storedDecisionKind(s: string): string {
  const k = s.split(":")[0];
  if (k === "deny") return "deny";
  if (k === "require_n_approvals") return "require_n_approvals";
  return "require_approval";
}
/// Extrae el N de un `require_n_approvals:N`. Default 2 (el backend rechaza n≤1).
function storedDecisionN(s: string): number {
  const parts = s.split(":");
  if (parts[0] !== "require_n_approvals") return 2;
  const n = parseInt(parts[1] ?? "", 10);
  return Number.isFinite(n) && n >= 2 ? n : 2;
}

const emptyForm = () => ({
  id: "",
  description: "",
  match_command: null as string | null,
  match_risk: null as string | null,
  match_agent_profile: null as string | null,
  match_plugin: null as string | null,
  decisionKind: "require_approval",
  nApprovals: 2,
});

export function PolicyView() {
  const [rules, setRules] = useState<StoredRule[]>([]);
  const [customEnabled, setCustomEnabled] = useState(false);
  const [msg, setMsg] = useState<string | null>(null);
  const [err, setErr] = useState<string | null>(null);
  const [form, setForm] = useState(emptyForm());
  const [editing, setEditing] = useState(false);
  const [preview, setPreview] = useState<PolicyPreview | null>(null);
  const [previewCmd, setPreviewCmd] = useState("");
  const [previewLoading, setPreviewLoading] = useState(false);

  const refresh = async () => {
    try {
      const list = await invoke<StoredRule[]>("policy_list_rules");
      setRules(list);
    } catch (e) {
      setErr(String(e));
    }
  };

  const refreshPreview = async () => {
    if (!previewCmd.trim()) return;
    setPreviewLoading(true);
    try {
      const p = await invoke<PolicyPreview>("policy_preview", {
        commandId: previewCmd.trim(),
        agentProfile: null,
        plugin: null,
      });
      setPreview(p);
      setCustomEnabled(p.custom_enabled);
    } catch (e) {
      setErr(String(e));
    } finally {
      setPreviewLoading(false);
    }
  };

  useEffect(() => {
    void refresh();
    // Cargar el estado real de custom_enabled al montar (audit-3 Codex: no esperar a un preview).
    invoke<boolean | null>("settings_get", { key: "policy.custom_enabled" })
      .then((v) => { if (typeof v === "boolean") setCustomEnabled(v); })
      .catch(() => { /* default false si el setting no existe */ });
  }, []);

  const toggleCustomEnabled = async (next: boolean) => {
    setErr(null); setMsg(null);
    try {
      await invoke("policy_set_custom_enabled", { enabled: next });
      setCustomEnabled(next);
      setMsg(`Reglas custom ${next ? "habilitadas" : "deshabilitadas"}.`);
    } catch (e) {
      setErr(String(e));
    }
  };

  const removeRule = async (id: string) => {
    if (!confirm(`¿Eliminar la regla "${id}"? Esta acción relaja el gobierno y requiere aprobación.`)) return;
    setErr(null); setMsg(null);
    try {
      await invoke("policy_remove_rule", { id });
      setMsg(`Regla "${id}" eliminada.`);
      await refresh();
    } catch (e) {
      setErr(String(e));
    }
  };

  const startEdit = (rule: StoredRule) => {
    setForm({
      id: rule.id,
      description: rule.description,
      match_command: rule.match_command,
      match_risk: rule.match_risk,
      match_agent_profile: rule.match_agent_profile,
      match_plugin: rule.match_plugin,
      decisionKind: storedDecisionKind(rule.decision),
      nApprovals: storedDecisionN(rule.decision),
    });
    setEditing(true);
    setMsg(rule.enabled ? null : "Nota: guardar esta regla la reactivará (el backend la habilita al upsert).");
    setErr(null);
  };

  const submitRule = async () => {
    if (!form.id.trim()) { setErr("El ID es obligatorio."); return; }
    if (!form.match_command && !form.match_risk && !form.match_agent_profile && !form.match_plugin) {
      setErr("Al menos un criterio de match debe estar seteado."); return;
    }
    setErr(null); setMsg(null);
    // CustomRule.decision es un enum TAGGED. require_n_approvals lleva el campo `n` (content="n").
    // El backend rechaza n≤1; forzamos mínimo 2 (n=1 ≡ require_approval).
    const decision = form.decisionKind === "require_n_approvals"
      ? { kind: "require_n_approvals", n: Math.max(2, form.nApprovals) }
      : { kind: form.decisionKind };
    try {
      await invoke("policy_set_rule", {
        rule: {
          id: form.id.trim(),
          description: form.description.trim(),
          match_command: form.match_command?.trim() || null,
          match_risk: form.match_risk || null,
          match_agent_profile: form.match_agent_profile?.trim() || null,
          match_plugin: form.match_plugin?.trim() || null,
          decision,
        },
      });
      setMsg(`Regla "${form.id}" guardada.`);
      setForm(emptyForm());
      setEditing(false);
      await refresh();
    } catch (e) {
      setErr(String(e));
    }
  };

  return (
    <div className="page policy-view">
      <div className="page-header">
        <div className="page-title">Policy — Reglas custom</div>
        <div className="page-sub">
          Hardening-only: solo endurece decisiones (Requiere aprobación / Denegar). Nunca relaja.
          {" "}{rules.length} regla(s) custom.
        </div>
      </div>

      {msg && <div className="toast-inline">{msg}</div>}
      {err && <div className="toast-inline" style={{ borderColor: "var(--danger, #d33)", color: "var(--danger, #d33)" }}>{err}</div>}

      {/* Toggle custom enabled */}
      <div style={{ display: "flex", alignItems: "center", gap: 12, marginBottom: 14, padding: "10px 14px", background: "var(--surface)", borderRadius: 8, border: "1px solid var(--border)" }}>
        <span style={{ fontSize: 13, fontWeight: 600 }}>Reglas custom:</span>
        <button
          className={`fxc-btn ${customEnabled ? "fxc-btn--danger" : ""}`}
          onClick={() => void toggleCustomEnabled(!customEnabled)}
        >
          {customEnabled ? "Habilitadas — Click para deshabilitar" : "Deshabilitadas — Click para habilitar"}
        </button>
        <span className="muted" style={{ fontSize: 12 }}>
          {customEnabled
            ? "Las reglas custom están activas y endurecen el gate."
            : "Las reglas existen pero no afectan el gate (default aplica)."}
        </span>
      </div>

      {/* Preview de un comando */}
      <div style={{ marginBottom: 16, padding: "10px 14px", background: "var(--surface)", borderRadius: 8, border: "1px solid var(--border)" }}>
        <div style={{ fontSize: 13, fontWeight: 600, marginBottom: 6 }}>Vista previa (¿qué decidiría el gate?)</div>
        <div style={{ display: "flex", gap: 8 }}>
          <input
            value={previewCmd}
            onChange={(e) => setPreviewCmd(e.target.value)}
            onKeyDown={(e) => e.key === "Enter" && void refreshPreview()}
            placeholder="command_id (ej: settings_set)"
            style={{ flex: 1, padding: "6px 10px", borderRadius: 6, border: "1px solid var(--border)", background: "var(--bg, #0e0e0e)", color: "var(--text)", fontSize: 13 }}
          />
          <button className="fxc-btn" onClick={() => void refreshPreview()} disabled={previewLoading || !previewCmd.trim()}>
            {previewLoading ? "…" : "Vista previa"}
          </button>
        </div>
        {preview && (
          <div style={{ marginTop: 8, fontSize: 12, display: "flex", gap: 16, flexWrap: "wrap" }}>
            <span>Default: <strong>{decisionLabel(preview.default_decision)}</strong></span>
            <span>Efectiva: <strong style={{ color: preview.hardened_by_custom ? "var(--amber, #f5a623)" : "inherit" }}>{decisionLabel(preview.effective_decision)}</strong></span>
            {preview.hardened_by_custom && <span className="sev-tag sev-warning">Endurecido por regla custom</span>}
            {!preview.hardened_by_custom && <span className="sev-tag sev-info">Sin cambio de custom</span>}
          </div>
        )}
      </div>

      {/* Tabla de reglas */}
      {rules.length === 0 ? (
        <div className="empty">
          <div className="head">Sin reglas custom</div>
          <div className="body muted">El gate usa solo las decisiones default del registry.</div>
        </div>
      ) : (
        <div style={{ overflowX: "auto", marginBottom: 16 }}>
          <table style={{ width: "100%", fontSize: 12, borderCollapse: "collapse" }}>
            <thead>
              <tr style={{ textAlign: "left", borderBottom: "1px solid var(--border)" }}>
                <th style={{ padding: "6px 8px" }}>ID</th>
                <th style={{ padding: "6px 8px" }}>Descripción</th>
                <th style={{ padding: "6px 8px" }}>Comando</th>
                <th style={{ padding: "6px 8px" }}>Risk</th>
                <th style={{ padding: "6px 8px" }}>Perfil</th>
                <th style={{ padding: "6px 8px" }}>Plugin</th>
                <th style={{ padding: "6px 8px" }}>Decisión</th>
                <th style={{ padding: "6px 8px" }}>Activa</th>
                <th style={{ padding: "6px 8px" }}>Acciones</th>
              </tr>
            </thead>
            <tbody>
              {rules.map((r) => (
                <tr key={r.id} style={{ borderBottom: "1px solid var(--border)", opacity: r.enabled ? 1 : 0.5 }}>
                  <td style={{ padding: "6px 8px", fontFamily: "var(--mono)", fontWeight: 600 }}>{r.id}</td>
                  <td style={{ padding: "6px 8px" }}>{r.description || <span className="muted">—</span>}</td>
                  <td style={{ padding: "6px 8px", fontFamily: "var(--mono)" }}>{r.match_command ?? <span className="muted">*</span>}</td>
                  <td style={{ padding: "6px 8px" }}>{r.match_risk ?? <span className="muted">*</span>}</td>
                  <td style={{ padding: "6px 8px" }}>{r.match_agent_profile ?? <span className="muted">*</span>}</td>
                  <td style={{ padding: "6px 8px" }}>{r.match_plugin ?? <span className="muted">*</span>}</td>
                  <td style={{ padding: "6px 8px" }}>
                    <span className={`sev-tag ${r.decision.startsWith("deny") ? "sev-critical" : "sev-warning"}`}>{decisionLabel(r.decision)}</span>
                  </td>
                  <td style={{ padding: "6px 8px" }}>
                    <span className={`sev-tag ${r.enabled ? "sev-info" : ""}`}>{r.enabled ? "sí" : "no"}</span>
                  </td>
                  <td style={{ padding: "6px 8px" }}>
                    <div style={{ display: "flex", gap: 4 }}>
                      <button className="fxc-btn" style={{ fontSize: 11, padding: "2px 8px" }} onClick={() => startEdit(r)}>Editar</button>
                      <button className="fxc-btn fxc-btn--danger" style={{ fontSize: 11, padding: "2px 8px" }} onClick={() => void removeRule(r.id)}>Eliminar</button>
                    </div>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}

      {/* Formulario agregar / editar */}
      <div style={{ padding: "12px 14px", background: "var(--surface)", borderRadius: 8, border: "1px solid var(--border)" }}>
        <div style={{ fontSize: 13, fontWeight: 600, marginBottom: 10 }}>
          {editing ? `Editando regla "${form.id}"` : "Agregar regla"}
        </div>
        <div style={{ display: "grid", gridTemplateColumns: "1fr 1fr", gap: 8 }}>
          <label style={{ fontSize: 12 }}>
            ID *
            <input
              value={form.id}
              onChange={(e) => setForm((f) => ({ ...f, id: e.target.value }))}
              placeholder="mi-regla"
              disabled={editing}
              style={{ display: "block", width: "100%", marginTop: 4, padding: "5px 8px", borderRadius: 5, border: "1px solid var(--border)", background: "var(--bg, #0e0e0e)", color: "var(--text)", fontSize: 12, boxSizing: "border-box" }}
            />
          </label>
          <label style={{ fontSize: 12 }}>
            Descripción
            <input
              value={form.description}
              onChange={(e) => setForm((f) => ({ ...f, description: e.target.value }))}
              placeholder="Para qué sirve esta regla"
              style={{ display: "block", width: "100%", marginTop: 4, padding: "5px 8px", borderRadius: 5, border: "1px solid var(--border)", background: "var(--bg, #0e0e0e)", color: "var(--text)", fontSize: 12, boxSizing: "border-box" }}
            />
          </label>
          <label style={{ fontSize: 12 }}>
            Match command (vacío = cualquiera)
            <input
              value={form.match_command ?? ""}
              onChange={(e) => setForm((f) => ({ ...f, match_command: e.target.value || null }))}
              placeholder="ej: settings_set"
              style={{ display: "block", width: "100%", marginTop: 4, padding: "5px 8px", borderRadius: 5, border: "1px solid var(--border)", background: "var(--bg, #0e0e0e)", color: "var(--text)", fontSize: 12, boxSizing: "border-box" }}
            />
          </label>
          <label style={{ fontSize: 12 }}>
            Match risk (vacío = cualquiera)
            <select
              value={form.match_risk ?? ""}
              onChange={(e) => setForm((f) => ({ ...f, match_risk: e.target.value || null }))}
              style={{ display: "block", width: "100%", marginTop: 4, padding: "5px 8px", borderRadius: 5, border: "1px solid var(--border)", background: "var(--bg, #0e0e0e)", color: "var(--text)", fontSize: 12, boxSizing: "border-box" }}
            >
              <option value="">— cualquiera —</option>
              {RISK_OPTIONS.map((r) => <option key={r.value} value={r.value}>{r.label}</option>)}
            </select>
          </label>
          <label style={{ fontSize: 12 }}>
            Match perfil de agente (vacío = cualquiera)
            <input
              value={form.match_agent_profile ?? ""}
              onChange={(e) => setForm((f) => ({ ...f, match_agent_profile: e.target.value || null }))}
              placeholder="ej: claude"
              style={{ display: "block", width: "100%", marginTop: 4, padding: "5px 8px", borderRadius: 5, border: "1px solid var(--border)", background: "var(--bg, #0e0e0e)", color: "var(--text)", fontSize: 12, boxSizing: "border-box" }}
            />
          </label>
          <label style={{ fontSize: 12 }}>
            Match plugin (vacío = cualquiera)
            <input
              value={form.match_plugin ?? ""}
              onChange={(e) => setForm((f) => ({ ...f, match_plugin: e.target.value || null }))}
              placeholder="ej: mi-plugin"
              style={{ display: "block", width: "100%", marginTop: 4, padding: "5px 8px", borderRadius: 5, border: "1px solid var(--border)", background: "var(--bg, #0e0e0e)", color: "var(--text)", fontSize: 12, boxSizing: "border-box" }}
            />
          </label>
          <label style={{ fontSize: 12 }}>
            Decisión *
            <select
              value={form.decisionKind}
              onChange={(e) => setForm((f) => ({ ...f, decisionKind: e.target.value }))}
              style={{ display: "block", width: "100%", marginTop: 4, padding: "5px 8px", borderRadius: 5, border: "1px solid var(--border)", background: "var(--bg, #0e0e0e)", color: "var(--text)", fontSize: 12, boxSizing: "border-box" }}
            >
              {DECISION_OPTIONS.map((d) => <option key={d.kind} value={d.kind}>{d.label}</option>)}
            </select>
          </label>
          {form.decisionKind === "require_n_approvals" && (
            <label style={{ fontSize: 12 }}>
              N aprobaciones (mínimo 2)
              <input
                type="number"
                min={2}
                value={form.nApprovals}
                onChange={(e) => setForm((f) => ({ ...f, nApprovals: Math.max(2, parseInt(e.target.value, 10) || 2) }))}
                style={{ display: "block", width: "100%", marginTop: 4, padding: "5px 8px", borderRadius: 5, border: "1px solid var(--border)", background: "var(--bg, #0e0e0e)", color: "var(--text)", fontSize: 12, boxSizing: "border-box" }}
              />
            </label>
          )}
        </div>
        <div style={{ display: "flex", gap: 8, marginTop: 12 }}>
          <button className="fxc-btn" onClick={() => void submitRule()}>
            {editing ? "Guardar cambios" : "Agregar regla"}
          </button>
          {editing && (
            <button className="fxc-btn" onClick={() => { setForm(emptyForm()); setEditing(false); setErr(null); }}>
              Cancelar
            </button>
          )}
        </div>
        <p className="muted" style={{ fontSize: 11, marginTop: 8, marginBottom: 0 }}>
          Hardening-only: las reglas solo pueden endurecer (Requiere aprobación / Denegar), nunca relajar.
          Al menos un criterio de match debe estar seteado.
        </p>
      </div>
    </div>
  );
}
