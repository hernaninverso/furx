// 047 FR-006 — Extensiones unificadas: una sola vista con tabs que fusiona el
// marketplace de Plugins (MCP firmados) y el de Skills (ex "Herramientas").
// Reduce la sobrecarga de vistas del sidebar (2 entradas → 1). NO reescribe los
// dos sub-paneles: los monta tal cual (PluginsView / ToolsView) bajo la tab activa.
//
// Deep-links: `furx://plugins` y `furx://tools` siguen vivos (el Shell mapea esas
// rutas a ESTA vista con la tab pre-seleccionada) → cero regresión de links viejos.
//
// Tokens V3 + patrón de tabs canónico (role=tablist/tab, aria-selected), dark+light.

import { useState } from "react";
import { PluginsView } from "./PluginsView";
import { ToolsView } from "./ToolsView";

export type ExtensionsTab = "plugins" | "skills";

const TABS: { id: ExtensionsTab; label: string; icon: string; hint: string }[] = [
  { id: "plugins", label: "Plugins", icon: "🧩", hint: "MCP firmados · sandbox net default-deny" },
  { id: "skills", label: "Skills", icon: "⚒", hint: "Skills instaladas · activar/correr/historial" },
];

export function ExtensionsView({ initialTab }: { initialTab?: ExtensionsTab } = {}) {
  const [tab, setTab] = useState<ExtensionsTab>(initialTab ?? "plugins");
  return (
    <div className="extensions-view" style={{ display: "flex", flexDirection: "column", height: "100%", minHeight: 0 }}>
      <div
        role="tablist"
        aria-label="Extensiones"
        className="extensions-tabs"
        style={{ display: "flex", gap: 4, padding: "10px 12px 0", flexWrap: "wrap" }}
      >
        {TABS.map((tb) => {
          const active = tb.id === tab;
          return (
            <button
              key={tb.id}
              id={`ext-tab-${tb.id}`}
              role="tab"
              aria-selected={active}
              aria-controls="ext-tabpanel"
              tabIndex={active ? 0 : -1}
              title={tb.hint}
              onClick={() => setTab(tb.id)}
              style={{
                padding: "6px 14px",
                borderRadius: 6,
                border: "1px solid var(--color-line)",
                background: active ? "var(--color-accent-dim)" : "transparent",
                color: active ? "var(--color-accent-bright)" : "var(--color-text-muted)",
                fontSize: "var(--fs-sm)",
                fontWeight: active ? 600 : 400,
                cursor: "pointer",
              }}
            >
              <span aria-hidden="true" style={{ marginRight: 6 }}>{tb.icon}</span>
              {tb.label}
            </button>
          );
        })}
      </div>
      <div
        id="ext-tabpanel"
        role="tabpanel"
        aria-labelledby={`ext-tab-${tab}`}
        className="extensions-panel"
        style={{ flex: 1, minHeight: 0, overflow: "auto" }}
      >
        {tab === "plugins" ? <PluginsView /> : <ToolsView />}
      </div>
    </div>
  );
}
