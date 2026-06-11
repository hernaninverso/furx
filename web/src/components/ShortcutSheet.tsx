import { useMemo, useState, useRef, useEffect } from "react";
import { Modal } from "./Modal";
import type { ActionEntry, ActionGroup } from "../actions";

const GROUP_ORDER: ActionGroup[] = ["Pane", "Modal", "View", "System"];

// 022 P0b (audit 3-frontera LOW) — sección "no-comando": keybindings GLOBALES del harness/modal
// (Esc, ⌘↩, Tab) que NO son comandos del registry (no se invocan vía `invoke(id)`; los maneja el
// overlay/modal o el palette). Por eso NO divergen del sidebar featured: los atajos que SÍ son
// comandos vienen TODOS de `buildActions()` (misma fuente que `featuredSidebarShortcuts`). Sólo los
// no-comando viven acá. Quedan etiquetados explícitamente (`isExtra`) para que sea evidente que no
// derivan del registry.
// TODO(post-022): si en el futuro estos keybindings se modelan como comandos del kernel (p.ej. un
// `ui.close_overlay` / `palette.cycle_mode`), moverlos al registry y borrar esta lista. Hoy NO son
// comandos → forzarlos al registry sería ruido (entradas no invocables). LOW, no bloquea.
const EXTRA_SHORTCUTS: { group: ActionGroup; label: string; hint?: string; shortcut: string }[] = [
  { group: "System", label: "Cerrar overlay actual", hint: "modal o palette", shortcut: "Esc" },
  { group: "System", label: "Submit modal (Council/Broadcast/...)", shortcut: "⌘↩" },
  { group: "System", label: "Tab cicla modos del palette", hint: "actions ↔ search ↔ project", shortcut: "Tab" },
];

export interface ShortcutSheetProps {
  actions: ActionEntry[];
  onClose: () => void;
}

interface DisplayRow {
  group: ActionGroup;
  label: string;
  hint?: string;
  shortcut: string;
  disabled: boolean;
  proGated: boolean;
  run?: () => void;
}

export function ShortcutSheet({ actions, onClose }: ShortcutSheetProps) {
  const [query, setQuery] = useState("");
  const inputRef = useRef<HTMLInputElement | null>(null);

  const rows: DisplayRow[] = useMemo(() => {
    const all: DisplayRow[] = [];
    for (const a of actions) {
      if (!a.shortcut) continue;
      const disabled = a.available ? !a.available() : false;
      all.push({
        group: a.group,
        label: a.label,
        hint: a.hint,
        shortcut: a.shortcut,
        disabled,
        proGated: !!a.proGated,
        run: a.run,
      });
    }
    for (const e of EXTRA_SHORTCUTS) {
      all.push({ ...e, disabled: false, proGated: false });
    }
    return all;
  }, [actions]);

  const filtered = useMemo(() => {
    const q = query.trim().toLowerCase();
    if (!q) return rows;
    return rows.filter((r) =>
      r.label.toLowerCase().includes(q) ||
      r.shortcut.toLowerCase().includes(q) ||
      (r.hint?.toLowerCase().includes(q) ?? false),
    );
  }, [rows, query]);

  const grouped: Record<ActionGroup, DisplayRow[]> = useMemo(() => {
    const out: Record<ActionGroup, DisplayRow[]> = {
      Pane: [], Modal: [], View: [], System: [],
    };
    for (const r of filtered) out[r.group].push(r);
    return out;
  }, [filtered]);

  const announceText = `${filtered.length} shortcut${filtered.length === 1 ? "" : "s"}`;

  // Codex MED: invoke synchronously inside onClose so the new modal (if any)
  // mounts before this Modal's focus-restore rAF runs. Modal also checks if
  // a portal already owns focus and skips restore in that case.
  const handleRun = (row: DisplayRow) => {
    if (row.disabled || !row.run) return;
    onClose();
    try { row.run(); } catch (e) { console.error("shortcut sheet action", e); }
  };

  // Auto-focus the search input on open (preferred over close-button).
  useEffect(() => {
    inputRef.current?.focus({ preventScroll: true });
  }, []);

  return (
    <Modal
      title="Keyboard shortcuts"
      subtitle={<>⌘/ · busque por nombre, hint o shortcut · click para ejecutar · Esc cierra</>}
      maxWidth={760}
      onClose={onClose}
      initialFocusRef={inputRef as React.RefObject<HTMLElement | null>}
    >
      <input
        ref={inputRef}
        value={query}
        onChange={(e) => setQuery(e.target.value)}
        placeholder="Buscar (label · shortcut · hint)…"
        aria-label="Filter shortcuts"
        className="shortcut-search"
      />
      <div className="sr-only" aria-live="polite" aria-atomic="true">{announceText}</div>
      <div className="shortcut-grid">
        {GROUP_ORDER.map((g) =>
          grouped[g].length === 0 ? null : (
            <section key={g} className="shortcut-group" aria-label={`${g} shortcuts`}>
              <header className="shortcut-group-header">{g}</header>
              <ul role="list" className="shortcut-list">
                {grouped[g].map((r, i) => (
                  <li key={`${g}-${i}-${r.shortcut}`} className={`shortcut-row ${r.disabled ? "is-disabled" : ""}`}>
                    <button
                      type="button"
                      className="shortcut-row-btn"
                      onClick={() => handleRun(r)}
                      disabled={!r.run || r.disabled}
                      aria-disabled={r.disabled || !r.run}
                      title={r.disabled ? "Disabled in current context" : undefined}
                    >
                      <span className="shortcut-label">{r.label}</span>
                      {r.proGated && <span className="pill-pro">PRO</span>}
                      {r.hint && <span className="shortcut-hint">{r.hint}</span>}
                      <kbd className="shortcut-key">{r.shortcut}</kbd>
                    </button>
                  </li>
                ))}
              </ul>
            </section>
          ),
        )}
        {filtered.length === 0 && <div className="muted shortcut-empty">No matches.</div>}
      </div>
    </Modal>
  );
}
