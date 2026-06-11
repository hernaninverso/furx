import { useCallback, useEffect, useMemo, useRef, useState, KeyboardEvent } from "react";
import { NavIcon } from "../lib/navIcons";

export type SidebarGroupId = "work" | "observability" | "infra" | "intelligence" | "extensions" | "system" | "config";

export interface SidebarItem<TView extends string = string> {
  view: TView;
  label: string;
  icon: string;
  badge?: string;
}

export interface SidebarGroupSpec<TView extends string = string> {
  id: SidebarGroupId;
  label: string;
  items: SidebarItem<TView>[];
}

const STORAGE_KEY = "furx.sidebar.groups.v1";

const DEFAULT_OPEN: Record<SidebarGroupId, boolean> = {
  work: true,
  observability: false,
  infra: false,
  intelligence: false,
  extensions: false,
  system: false,
  config: false,
};

const VALID_IDS: SidebarGroupId[] = ["work", "observability", "infra", "intelligence", "extensions", "system", "config"];

function readStored(): Record<SidebarGroupId, boolean> {
  try {
    const raw = typeof localStorage !== "undefined" ? localStorage.getItem(STORAGE_KEY) : null;
    if (!raw) return { ...DEFAULT_OPEN };
    const parsed = JSON.parse(raw);
    if (!parsed || typeof parsed !== "object") return { ...DEFAULT_OPEN };
    const merged = { ...DEFAULT_OPEN };
    for (const id of VALID_IDS) {
      if (typeof parsed[id] === "boolean") merged[id] = parsed[id];
    }
    return merged;
  } catch {
    return { ...DEFAULT_OPEN };
  }
}

function writePatch(patch: Partial<Record<SidebarGroupId, boolean>>): void {
  try {
    if (typeof localStorage === "undefined") return;
    // Codex MED (v2): only persist the keys actually changed in this tab so a
    // sibling tab's recent toggle of another group isn't clobbered.
    const latest = readStored();
    let changed = false;
    for (const k of VALID_IDS) {
      const v = patch[k];
      if (typeof v === "boolean" && latest[k] !== v) {
        latest[k] = v;
        changed = true;
      }
    }
    if (!changed) return;
    localStorage.setItem(STORAGE_KEY, JSON.stringify(latest));
  } catch {
    // Quota / disabled / privacy mode — silent.
  }
}

function clampBadge(n: number): string | null {
  if (!Number.isFinite(n) || n <= 0) return null;
  if (n > 99) return "99+";
  return String(n);
}

function aggregateBadge(items: SidebarItem[]): string | null {
  let sum = 0;
  let hasNumeric = false;
  for (const it of items) {
    if (!it.badge) continue;
    const n = Number(it.badge);
    if (Number.isFinite(n)) {
      sum += n;
      hasNumeric = true;
    }
  }
  if (!hasNumeric) return null;
  return clampBadge(sum);
}

interface SidebarGroupProps<TView extends string> {
  spec: SidebarGroupSpec<TView>;
  open: boolean;
  activeView: TView;
  onToggle: (id: SidebarGroupId) => void;
  onSelect: (view: TView) => void;
  onFocusNeighbor: (id: SidebarGroupId, dir: "prev" | "next") => void;
}

function SidebarGroup<TView extends string>({ spec, open, activeView, onToggle, onSelect, onFocusNeighbor }: SidebarGroupProps<TView>) {
  const headerRef = useRef<HTMLButtonElement | null>(null);
  const childrenRef = useRef<HTMLDivElement | null>(null);
  // Codex LOW: aggregate badge uses effective state (open === effectiveOpen now
  // because forceOpen-on-view-change persists storage in the parent).
  const aggBadge = open ? null : aggregateBadge(spec.items);

  const handleHeaderKey = useCallback(
    (e: KeyboardEvent<HTMLButtonElement>) => {
      if (e.key === "ArrowDown") {
        if (open) {
          const first = childrenRef.current?.querySelector<HTMLButtonElement>("button.nav-btn");
          if (first) {
            e.preventDefault();
            first.focus();
            return;
          }
        }
        // Closed or empty → move to next group's header.
        e.preventDefault();
        onFocusNeighbor(spec.id, "next");
      } else if (e.key === "ArrowUp") {
        e.preventDefault();
        onFocusNeighbor(spec.id, "prev");
      }
    },
    [open, onFocusNeighbor, spec.id],
  );

  const handleChildKey = useCallback(
    (e: KeyboardEvent<HTMLDivElement>) => {
      if (e.key !== "ArrowUp" && e.key !== "ArrowDown") return;
      const target = e.target as HTMLElement;
      if (!target || target.tagName !== "BUTTON") return;
      const buttons = Array.from(
        childrenRef.current?.querySelectorAll<HTMLButtonElement>("button.nav-btn") ?? [],
      );
      const idx = buttons.indexOf(target as HTMLButtonElement);
      if (idx === -1) return;
      if (e.key === "ArrowUp") {
        if (idx === 0) {
          e.preventDefault();
          headerRef.current?.focus();
        } else {
          e.preventDefault();
          buttons[idx - 1].focus();
        }
      } else {
        if (idx < buttons.length - 1) {
          e.preventDefault();
          buttons[idx + 1].focus();
        } else {
          // Last child → next group header.
          e.preventDefault();
          onFocusNeighbor(spec.id, "next");
        }
      }
    },
    [onFocusNeighbor, spec.id],
  );

  const handleToggle = useCallback(() => {
    onToggle(spec.id);
  }, [onToggle, spec.id]);

  // Return focus to header if children become hidden while focus is inside.
  useEffect(() => {
    if (open) return;
    const active = typeof document !== "undefined" ? document.activeElement : null;
    if (active && childrenRef.current && childrenRef.current.contains(active)) {
      headerRef.current?.focus();
    }
  }, [open]);

  const childId = `sidebar-group-${spec.id}`;
  return (
    <div className="sidebar-group" data-open={open ? "true" : "false"} data-group-id={spec.id}>
      <button
        ref={headerRef}
        type="button"
        className="sidebar-group-header"
        onClick={handleToggle}
        onKeyDown={handleHeaderKey}
        aria-expanded={open}
        aria-controls={childId}
      >
        <span className="sidebar-group-chevron" aria-hidden="true">▸</span>
        <span className="sidebar-group-label">{spec.label}</span>
        {aggBadge && <span className="badge badge-sm">{aggBadge}</span>}
      </button>
      <div
        ref={childrenRef}
        id={childId}
        className="sidebar-group-children"
        role="group"
        aria-label={spec.label}
        // Codex MED: belt-and-suspenders for older WebKit where `inert` isn't honored:
        // also set aria-hidden so AT skips collapsed children.
        aria-hidden={!open}
        inert={!open}
        onKeyDown={handleChildKey}
      >
        <div className="sidebar-group-children-inner">
          {spec.items.map((item) => (
            <button
              key={item.view}
              type="button"
              className={`nav-btn ${activeView === item.view ? "active" : ""}`}
              onClick={() => onSelect(item.view)}
              tabIndex={open ? 0 : -1}
              aria-current={activeView === item.view ? "page" : undefined}
            >
              <span className="nav-icon" aria-hidden="true"><NavIcon view={item.view} /></span>
              <span>{item.label}</span>
              {item.badge && <span className="badge">{item.badge}</span>}
            </button>
          ))}
        </div>
      </div>
    </div>
  );
}

export function findGroupFor(activeView: string, groups: SidebarGroupSpec[]): SidebarGroupId | null {
  for (const g of groups) {
    if (g.items.some((it) => it.view === activeView)) return g.id;
  }
  return null;
}

interface SidebarGroupsProps<TView extends string> {
  groups: SidebarGroupSpec<TView>[];
  activeView: TView;
  onSelect: (view: TView) => void;
  /// 015 T020: si `false`, ROLLBACK a una lista PLANA de todas las vistas (sin grupos). Default
  /// `true` (agrupada). El Shell lo cablea al flag `groupedNav`.
  grouped?: boolean;
}

export function SidebarGroups<TView extends string>({ groups, activeView, onSelect, grouped = true }: SidebarGroupsProps<TView>) {
  const [openMap, setOpenMap] = useState<Record<SidebarGroupId, boolean>>(() => readStored());
  const navRef = useRef<HTMLElement | null>(null);

  // Cross-tab / cross-window sync.
  useEffect(() => {
    if (typeof window === "undefined") return;
    const handler = (e: StorageEvent) => {
      if (e.key !== STORAGE_KEY) return;
      setOpenMap(readStored());
    };
    window.addEventListener("storage", handler);
    return () => window.removeEventListener("storage", handler);
  }, []);

  const activeGroup = useMemo(() => findGroupFor(activeView, groups), [activeView, groups]);

  // Gemini HIGH: keep state updaters pure. State→storage sync lives in its own
  // effect that diffs against the previous render to write only changed keys.
  const previousMapRef = useRef<Record<SidebarGroupId, boolean> | null>(null);
  useEffect(() => {
    const prev = previousMapRef.current;
    previousMapRef.current = openMap;
    if (prev === null) return; // initial render — already from storage.
    const patch: Partial<Record<SidebarGroupId, boolean>> = {};
    for (const k of VALID_IDS) {
      if (prev[k] !== openMap[k]) patch[k] = openMap[k];
    }
    if (Object.keys(patch).length > 0) writePatch(patch);
  }, [openMap]);

  // Codex LOW: persist auto-expand on view change so the user can later collapse
  // even while sitting on that view. setState is now pure; storage sync runs in
  // the effect above.
  useEffect(() => {
    if (!activeGroup) return;
    setOpenMap((prev) => (prev[activeGroup] ? prev : { ...prev, [activeGroup]: true }));
  }, [activeGroup]);

  const handleToggle = useCallback((id: SidebarGroupId) => {
    setOpenMap((prev) => ({ ...prev, [id]: !prev[id] }));
  }, []);

  const renderable = useMemo(() => groups.filter((g) => g.items.length > 0), [groups]);

  const handleFocusNeighbor = useCallback(
    (id: SidebarGroupId, dir: "prev" | "next") => {
      const idx = renderable.findIndex((g) => g.id === id);
      if (idx === -1) return;
      const target = dir === "next" ? renderable[idx + 1] : renderable[idx - 1];
      if (!target) return;
      const nav = navRef.current;
      if (!nav) return;
      if (dir === "prev" && openMap[target.id]) {
        // Land on last visible child of the previous group, not its header.
        const buttons = nav.querySelectorAll<HTMLButtonElement>(
          `.sidebar-group[data-group-id="${target.id}"] button.nav-btn`,
        );
        const last = buttons[buttons.length - 1];
        if (last) {
          last.focus();
          return;
        }
      }
      const header = nav.querySelector<HTMLButtonElement>(
        `.sidebar-group[data-group-id="${target.id}"] > .sidebar-group-header`,
      );
      header?.focus();
    },
    [renderable, openMap],
  );

  // 015 T020 — ROLLBACK: con el flag agrupado OFF, renderizamos TODAS las vistas en una lista
  // plana (sin grupos colapsables). No es un componente aparte: son los MISMOS items de los
  // grupos, aplanados → cero pérdida de cobertura, rollback instantáneo.
  if (!grouped) {
    const flatItems = groups.flatMap((g) => g.items);
    return (
      <nav className="nav nav-flat" aria-label="Primary" ref={navRef}>
        {flatItems.map((item) => (
          <button
            key={item.view}
            type="button"
            className={`nav-btn ${activeView === item.view ? "active" : ""}`}
            onClick={() => onSelect(item.view)}
            aria-current={activeView === item.view ? "page" : undefined}
          >
            <span className="nav-icon" aria-hidden="true">{item.icon}</span>
            <span>{item.label}</span>
            {/* 015 T020 (audit MED): preservar los badges en modo flat (rollback) — el conteo de
                incidentes/paneles/auditoría/monitores no debe perderse al apagar la agrupación. */}
            {item.badge && <span className="badge">{item.badge}</span>}
          </button>
        ))}
      </nav>
    );
  }

  return (
    <nav className="nav nav-grouped" aria-label="Primary" ref={navRef}>
      {renderable.map((g) => (
        <SidebarGroup
          key={g.id}
          spec={g}
          open={!!openMap[g.id]}
          activeView={activeView}
          onToggle={handleToggle}
          onSelect={onSelect}
          onFocusNeighbor={handleFocusNeighbor}
        />
      ))}
    </nav>
  );
}

// Exposed for unit tests.
export const __test__ = {
  STORAGE_KEY,
  DEFAULT_OPEN,
  readStored,
  writePatch,
  aggregateBadge,
  clampBadge,
};
