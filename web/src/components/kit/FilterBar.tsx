// 019 F2 T020 — FilterBar: barra de filtro reusable (texto + facetas tipo chip). PRESENTACIÓN pura;
// la lógica de matching vive en `lib/kit/filter.ts` (testeada sin DOM). Reusada en ≥3 superficies
// (audit, queue, eval…). Tokens V3, dark+light. Sin "honest/honesto".
import type { Facet, FilterState } from "../../lib/kit/filter";
import { kitInput, kitLbl, kitChip } from "./styles";

export function FilterBar({
  state, onChange, facets = [], placeholder = "filtrar…", autoFocus,
}: {
  state: FilterState;
  onChange: (next: FilterState) => void;
  facets?: Facet[];
  placeholder?: string;
  autoFocus?: boolean;
}) {
  const setQuery = (query: string) => onChange({ ...state, query });
  const setFacet = (facetId: string, value: string | null) =>
    onChange({ ...state, facets: { ...state.facets, [facetId]: value } });

  return (
    <div style={{ display: "flex", flexDirection: "column", gap: 8 }}>
      <input
        type="search"
        style={kitInput}
        value={state.query}
        placeholder={placeholder}
        autoFocus={autoFocus}
        aria-label="Filtrar por texto"
        onChange={(e) => setQuery(e.target.value)}
      />
      {facets.map((f) => {
        const active = state.facets[f.id] ?? null;
        return (
          <div key={f.id} style={{ display: "flex", flexWrap: "wrap", gap: 6, alignItems: "center" }}>
            <span style={{ ...kitLbl, marginRight: 2 }}>{f.label}</span>
            <button
              type="button"
              style={kitChip(active === null)}
              aria-pressed={active === null}
              onClick={() => setFacet(f.id, null)}
            >
              todos
            </button>
            {f.options.map((o) => (
              <button
                key={o.value}
                type="button"
                style={kitChip(active === o.value)}
                aria-pressed={active === o.value}
                onClick={() => setFacet(f.id, active === o.value ? null : o.value)}
              >
                {o.label}{o.count != null ? ` · ${o.count}` : ""}
              </button>
            ))}
          </div>
        );
      })}
    </div>
  );
}
