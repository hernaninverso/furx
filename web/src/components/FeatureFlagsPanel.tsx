// web/src/components/FeatureFlagsPanel.tsx — 015 T022 (FR-014) · UI de feature-flags locales.
//
// Lista dinámicamente el registry `FLAGS` (lib/flags) y togglea cada uno con `useFlag`. Los flags
// cuya feature aún NO está implementada (`impl:false`) se muestran DISABLED con "próximamente" —
// honestidad: no dejamos que el usuario active algo que no hace nada. Vive en Settings → Avanzado.

import { FLAGS, FlagDef, FlagName, useFlag } from "../lib/flags";

function FlagRow({ name }: { name: FlagName }) {
  const def: FlagDef = FLAGS[name];
  const [value, setValue] = useFlag(name);
  return (
    <label
      className="flag-row"
      style={{ display: "flex", alignItems: "flex-start", gap: 8, padding: "6px 0", opacity: def.impl ? 1 : 0.6 }}
    >
      <input
        type="checkbox"
        checked={value}
        disabled={!def.impl}
        onChange={(e) => setValue(e.target.checked)}
        style={{ marginTop: 3 }}
      />
      <span>
        <strong>{def.label}</strong>
        {!def.impl && <span className="muted"> · próximamente</span>}
        {def.impl && def.beta && (
          <span
            className="muted"
            title="Implementado y testeado, pero la validación en vivo con 2 monitores aún no se corrió. Activá bajo tu propio criterio."
            style={{ fontSize: 11, fontWeight: 600, letterSpacing: 0.4, textTransform: "uppercase" }}
          > · beta</span>
        )}
        {def.description && (
          <div className="muted" style={{ fontSize: 12 }}>{def.description}</div>
        )}
        {def.impl && def.beta && (
          <div className="muted" style={{ fontSize: 11, fontStyle: "italic" }}>
            Experimental: probá detach y multi-monitor antes de depender de esta función.
          </div>
        )}
      </span>
    </label>
  );
}

export function FeatureFlagsPanel() {
  const names = Object.keys(FLAGS) as FlagName[];
  return (
    <div className="feature-flags">
      <div className="muted" style={{ marginBottom: 8, fontSize: 12 }}>
        Flags locales (sólo esta máquina). Los marcados "próximamente" aún no hacen nada.
      </div>
      {names.map((n) => (
        <FlagRow key={n} name={n} />
      ))}
    </div>
  );
}
