// US7 — Settings UI generada del registry, con search (spec 015-frontend-reform-kernel).
//
// Componente INDEPENDIENTE (no reescribe el Settings.tsx viejo). Carga el
// registry curado desde Rust (`settings_registry_list`), renderiza tabs por
// dominio + un search box, y un control por setting según su schema
// (toggle/select/input/number). Escribe vía `settings_set_validated` que valida
// el schema en el backend. La validación cliente da feedback instantáneo.
//
// Se integrará en la nav en otra ola; por ahora es montable standalone.

import { useEffect, useMemo, useState } from "react";
import type { ReactNode } from "react";
import {
  DOMAIN_LABELS,
  DOMAIN_ORDER,
  loadRegistry,
  loadValues,
  searchSettings,
  setValidated,
  validateValue,
  type SettingDef,
  type SettingDomain,
} from "../lib/settingsRegistry";

const RISK_COLOR: Record<SettingDef["risk"], string> = {
  Safe: "var(--color-text-muted)",
  Caution: "var(--color-warning)",
  Destructive: "var(--color-danger)",
};

export function SettingsRegistryPanel(): ReactNode {
  const [defs, setDefs] = useState<SettingDef[]>([]);
  const [values, setValues] = useState<Record<string, unknown>>({});
  const [query, setQuery] = useState("");
  const [activeDomain, setActiveDomain] = useState<SettingDomain | null>(null);
  const [showAdvanced, setShowAdvanced] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [loaded, setLoaded] = useState(false);

  const refresh = async () => {
    const [r, v] = await Promise.all([loadRegistry(), loadValues()]);
    setDefs(r);
    setValues(v);
    setLoaded(true);
  };

  useEffect(() => {
    refresh().catch((e) => setError(String(e)));
  }, []);

  // Domains that actually have visible settings (skip Internal-only).
  const visibleDefs = useMemo(
    () => defs.filter((d) => d.visibility !== "Internal" && (showAdvanced || d.visibility === "Visible")),
    [defs, showAdvanced],
  );

  const domains = useMemo(
    () => DOMAIN_ORDER.filter((dom) => visibleDefs.some((d) => d.domain === dom)),
    [visibleDefs],
  );

  // While searching, ignore the active-domain tab and search across all.
  const searching = query.trim().length > 0;
  const shown = useMemo(() => {
    const base = searchSettings(visibleDefs, query);
    if (searching) return base;
    const dom = activeDomain ?? domains[0] ?? null;
    return dom ? base.filter((d) => d.domain === dom) : base;
  }, [visibleDefs, query, searching, activeDomain, domains]);

  const valueFor = (d: SettingDef): unknown =>
    d.key in values ? values[d.key] : d.default_value;

  const onChange = async (d: SettingDef, raw: unknown) => {
    setError(null);
    // Client-side validation for instant feedback.
    const clientErr = validateValue(d.schema, raw);
    if (clientErr) {
      setError(`${d.label}: ${clientErr}`);
      return;
    }
    // Optimistic update.
    setValues((prev) => ({ ...prev, [d.key]: raw }));
    try {
      await setValidated(d.key, raw);
    } catch (e) {
      // Backend rejected — revert and surface.
      setError(`${d.label}: ${String(e)}`);
      await refresh().catch(() => {});
    }
  };

  if (error && !loaded) {
    return (
      <div style={{ padding: 16, color: "var(--color-danger)" }}>
        Failed to load settings registry: {error}
      </div>
    );
  }

  return (
    <div className="settings-registry-panel" style={{ display: "flex", flexDirection: "column", gap: 12, padding: 16 }}>
      <input
        type="search"
        placeholder="Search settings…"
        value={query}
        onChange={(e) => setQuery(e.target.value)}
        aria-label="Search settings"
        style={{
          padding: "8px 12px",
          background: "var(--color-bg-elevated)",
          border: "1px solid var(--color-line)",
          borderRadius: 6,
          color: "var(--color-text)",
          fontSize: "var(--fs-sm)",
        }}
      />

      {!searching && (
        <div role="tablist" style={{ display: "flex", gap: 4, flexWrap: "wrap" }}>
          {domains.map((dom) => {
            const active = (activeDomain ?? domains[0]) === dom;
            return (
              <button
                key={dom}
                role="tab"
                aria-selected={active}
                onClick={() => setActiveDomain(dom)}
                style={{
                  padding: "4px 10px",
                  borderRadius: 6,
                  border: "1px solid var(--color-line)",
                  background: active ? "var(--color-accent-dim)" : "transparent",
                  color: active ? "var(--color-accent-bright)" : "var(--color-text-muted)",
                  fontSize: "var(--fs-xs)",
                  cursor: "pointer",
                }}
              >
                {DOMAIN_LABELS[dom]}
              </button>
            );
          })}
        </div>
      )}

      <label style={{ display: "flex", alignItems: "center", gap: 6, fontSize: "var(--fs-xs)", color: "var(--color-text-muted)" }}>
        <input type="checkbox" checked={showAdvanced} onChange={(e) => setShowAdvanced(e.target.checked)} />
        Show advanced settings
      </label>

      {error && (
        <div role="alert" style={{ color: "var(--color-danger)", fontSize: "var(--fs-xs)" }}>
          {error}
        </div>
      )}

      <div style={{ display: "flex", flexDirection: "column", gap: 10 }}>
        {shown.length === 0 ? (
          <div style={{ color: "var(--color-text-faint)", fontSize: "var(--fs-sm)" }}>
            No settings match “{query}”.
          </div>
        ) : (
          shown.map((d) => (
            <SettingRow key={d.key} def={d} value={valueFor(d)} onChange={(v) => onChange(d, v)} />
          ))
        )}
      </div>
    </div>
  );
}

function SettingRow({
  def,
  value,
  onChange,
}: {
  def: SettingDef;
  value: unknown;
  onChange: (v: unknown) => void;
}): ReactNode {
  return (
    <div
      data-setting-key={def.key}
      style={{
        display: "flex",
        justifyContent: "space-between",
        alignItems: "flex-start",
        gap: 16,
        padding: "8px 0",
        borderBottom: "1px solid var(--color-line)",
      }}
    >
      <div style={{ flex: 1, minWidth: 0 }}>
        <div style={{ display: "flex", alignItems: "center", gap: 8 }}>
          <span style={{ color: "var(--color-text)", fontSize: "var(--fs-sm)", fontWeight: "var(--fw-medium)" }}>
            {def.label}
          </span>
          {def.risk !== "Safe" && (
            <span style={{ color: RISK_COLOR[def.risk], fontSize: "var(--fs-xs)" }}>● {def.risk}</span>
          )}
          {def.restart_required && (
            <span style={{ color: "var(--color-text-faint)", fontSize: "var(--fs-xs)" }}>restart</span>
          )}
        </div>
        <div style={{ color: "var(--color-text-muted)", fontSize: "var(--fs-xs)", marginTop: 2 }}>
          {def.description}
        </div>
        <div style={{ color: "var(--color-text-faint)", fontSize: "var(--fs-xs)", fontFamily: "var(--font-mono)", marginTop: 2 }}>
          {def.key}
        </div>
      </div>
      <div style={{ flexShrink: 0 }}>
        <SettingControl def={def} value={value} onChange={onChange} />
      </div>
    </div>
  );
}

function SettingControl({
  def,
  value,
  onChange,
}: {
  def: SettingDef;
  value: unknown;
  onChange: (v: unknown) => void;
}): ReactNode {
  const inputStyle = {
    padding: "4px 8px",
    background: "var(--color-bg-elevated)",
    border: "1px solid var(--color-line)",
    borderRadius: 4,
    color: "var(--color-text)",
    fontSize: "var(--fs-xs)",
  } as const;

  switch (def.schema.type) {
    case "bool":
      return (
        <input
          type="checkbox"
          checked={value === true}
          aria-label={def.label}
          onChange={(e) => onChange(e.target.checked)}
        />
      );
    case "enum":
      return (
        <select
          value={typeof value === "string" ? value : ""}
          aria-label={def.label}
          onChange={(e) => onChange(e.target.value)}
          style={inputStyle}
        >
          {def.schema.options.map((opt) => (
            <option key={opt} value={opt}>
              {opt}
            </option>
          ))}
        </select>
      );
    case "number":
      return (
        <input
          type="number"
          value={typeof value === "number" ? value : ""}
          aria-label={def.label}
          min={def.schema.min ?? undefined}
          max={def.schema.max ?? undefined}
          step="any"
          onChange={(e) => {
            const n = e.target.value === "" ? NaN : Number(e.target.value);
            onChange(Number.isNaN(n) ? "" : n);
          }}
          style={{ ...inputStyle, width: 90 }}
        />
      );
    case "string":
    default:
      return (
        <input
          type="text"
          value={typeof value === "string" ? value : ""}
          aria-label={def.label}
          maxLength={def.schema.type === "string" ? def.schema.max_len ?? undefined : undefined}
          onChange={(e) => onChange(e.target.value)}
          style={{ ...inputStyle, width: 220 }}
        />
      );
  }
}
