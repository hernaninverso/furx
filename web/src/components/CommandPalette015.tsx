/* CommandPalette015 — ⌘K universal command palette (US2, spec 015).
 *
 * Indexa el Command Registry (US1, `commandRegistry.ts` → comando Tauri
 * `command_registry_list`), ofrece fuzzy search + filtros (dominio/scope/risk)
 * y ejecuta comandos. Reglas (constitución VI · F-VI):
 *   - Enter ejecuta `safe` directo (`invoke(command_id)`).
 *   - Comandos con `requires_confirmation`, risk `destructive` o `credential`
 *     pasan por una confirmación (DangerZone canónico) ANTES de invocar.
 *   - Comandos con `deeplink` navegan vía callback `onNavigate` (el router real
 *     es US9; acá sólo delegamos el string furx://...).
 *   - Visibilidad: por defecto sólo `primary`/`palette`; un toggle "advanced"
 *     revela `internal`. `hidden` NUNCA se muestra (oculto de toda superficie).
 *
 * Estado window-scoped: todo el estado (query/selección/confirm) es local de
 * ESTE componente — NO hay singleton global, así cada ventana tiene su palette
 * (multi-window-ready). Reusa los componentes canónicos ModalFrame + CommandRow
 * + DangerZone y SOLO tokens (estética V3 atelier).
 */

import { useEffect, useMemo, useRef, useState } from "react";
// 015 T015 — invoke envuelto: el gate universal del backend + el modal GLOBAL de aprobación se
// encargan de los comandos Destructive/Credential. El palette sólo invoca; no maneja aprobación.
import { invoke } from "../lib/invoke";
import { ModalFrame, CommandRow } from "./canonical";
import {
  CommandDef,
  CommandRisk,
  CommandScope,
  loadCommandRegistry,
  usablePaletteCommands,
} from "../lib/commandRegistry";
// 015 T013 (US9) — entradas de NAVEGACIÓN sintéticas (vistas + secciones de Settings). NO son
// comandos Tauri: el palette las ofrece para deep-linkear vía el router interno (`onNavigate`).
import { navTargets } from "../lib/router";

/// Convierte los destinos de navegación del router en CommandDef sintéticos (deeplink seteado,
/// id namespaceado `nav.*` para no colisionar con ids de comandos reales del registry).
function navCommandDefs(): CommandDef[] {
  return navTargets().map((t) => ({
    id: `nav.${t.route.replace(/^furx:\/\//, "").replace(/\//g, ".")}`,
    label: t.label,
    description: "",
    category: "navigation",
    scope: "view" as CommandScope,
    risk: "safe" as CommandRisk,
    visibility: "palette",
    shortcut: null,
    requires_confirmation: false,
    reversible: true,
    deeplink: t.route,
    extra: {},
  }));
}

/**
 * 047 FR-001 — acción CONTEXTUAL por vista, inyectada por el Shell según el `view` activo (o
 * acciones globales movidas del top bar: PR Description, Disagree). NO es un comando Tauri: el
 * palette la ofrece arriba y, al elegirla, ejecuta `run()` (que abre un modal / navega) y cierra.
 * Las acciones son aperturas/navegación humanas explícitas — NUNCA auto-disparan nada.
 */
export interface ContextAction {
  /** id único namespaceado `ctx.*` para no colisionar con comandos reales del registry. */
  id: string;
  label: string;
  /** descripción corta opcional (cae al id si falta). */
  description?: string;
  /** etiqueta del origen para el subgrupo (ej "Esta vista", "Acciones"). */
  group?: string;
  run: () => void;
}

export interface CommandPalette015Props {
  /** Cerrar el palette (ESC, backdrop, ×, post-ejecución). */
  onClose: () => void;
  /**
   * Resolver un deeplink (furx://...). El router real es US9; por ahora el Shell
   * decide qué hacer. Si no se pasa, los comandos con deeplink se ejecutan como
   * un comando normal (fallback al invoke del id).
   */
  onNavigate?: (deeplink: string) => void;
  /**
   * 047 FR-001 — acciones contextuales de la vista activa + las movidas del top bar.
   * Aparecen ARRIBA (categoría `context`) y son searchables como cualquier comando.
   */
  contextActions?: ContextAction[];
}

/* ── Fuzzy match ─────────────────────────────────────────────────────────
 * Subsequence scorer simple (sin dependencia): cada char de la query debe
 * aparecer en orden en el haystack. Premia matches contiguos y al inicio de
 * palabra. Devuelve null si no matchea (para filtrar). Mayor score = mejor.
 */
function fuzzyScore(query: string, text: string): number | null {
  if (query.length === 0) return 0;
  const q = query.toLowerCase();
  const t = text.toLowerCase();
  let qi = 0;
  let score = 0;
  let prevIdx = -1;
  for (let ti = 0; ti < t.length && qi < q.length; ti++) {
    if (t[ti] === q[qi]) {
      // bonus por contigüidad y por boundary (inicio o tras separador).
      if (prevIdx === ti - 1) score += 6;
      const atBoundary = ti === 0 || /[\s/_.\-:]/.test(t[ti - 1]);
      if (atBoundary) score += 4;
      score += 1;
      prevIdx = ti;
      qi++;
    }
  }
  return qi === q.length ? score : null;
}

/** Mejor score del comando sobre label/id/category (el campo más fuerte gana). */
function commandScore(query: string, c: CommandDef): number | null {
  if (query.trim() === "") return 0;
  const label = fuzzyScore(query, c.label);
  const id = fuzzyScore(query, c.id);
  const cat = fuzzyScore(query, c.category);
  const scores = [label, id != null ? id - 2 : null, cat != null ? cat - 4 : null].filter(
    (s): s is number => s != null,
  );
  if (scores.length === 0) return null;
  return Math.max(...scores);
}

const SCOPE_FILTERS: { value: CommandScope | "all"; label: string }[] = [
  { value: "all", label: "all scopes" },
  { value: "app", label: "app" },
  { value: "window", label: "window" },
  { value: "view", label: "view" },
  { value: "pane", label: "pane" },
];

const RISK_FILTERS: { value: CommandRisk | "all"; label: string }[] = [
  { value: "all", label: "all risk" },
  { value: "safe", label: "safe" },
  { value: "destructive", label: "destructive" },
  { value: "credential", label: "credential" },
  { value: "external", label: "external" },
];

/** Glifo corto por categoría (degrada a 2 letras). Sólo presentación. */
function categoryGlyph(category: string): string {
  return category.slice(0, 2).toUpperCase() || "··";
}

/// 047 FR-001 — convierte las acciones contextuales en CommandDef sintéticos (category `context`,
/// scope `view`, riesgo `safe`). El `run` real se guarda aparte (map id→run) y lo dispara executeNow.
function contextCommandDefs(actions: ContextAction[]): CommandDef[] {
  return actions.map((a) => ({
    id: a.id,
    label: a.label,
    description: a.description ?? "",
    category: "context",
    scope: "view" as CommandScope,
    risk: "safe" as CommandRisk,
    visibility: "palette",
    shortcut: null,
    requires_confirmation: false,
    reversible: true,
    deeplink: null,
    extra: a.group ? { group: a.group } : {},
  }));
}

export function CommandPalette015({ onClose, onNavigate, contextActions }: CommandPalette015Props) {
  // ── Estado window-scoped (local del componente, NO singleton). ──────────
  const [all, setAll] = useState<CommandDef[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  const [query, setQuery] = useState("");
  const [domain, setDomain] = useState<string>("all");
  const [scope, setScope] = useState<CommandScope | "all">("all");
  const [risk, setRisk] = useState<CommandRisk | "all">("all");
  const [showAdvanced, setShowAdvanced] = useState(false);
  const [active, setActive] = useState(0);

  const [running, setRunning] = useState(false);
  const [runError, setRunError] = useState<string | null>(null);

  const inputRef = useRef<HTMLInputElement>(null);
  const listRef = useRef<HTMLDivElement>(null);

  // Cargar el registry una vez al montar.
  useEffect(() => {
    let alive = true;
    loadCommandRegistry()
      .then((cmds) => {
        if (!alive) return;
        // 015 T013: mergeamos los comandos reales (del registry Rust) con las entradas de
        // navegación sintéticas (front-only) → el palette navega Y ejecuta desde un solo índice.
        // Dedup defensivo (audit LOW): si un id real ya existe (hoy ninguno con punto, pero a
        // prueba de futuro), la entrada nav NO lo sombrea.
        // P0b (audit 3-frontera HIGH 2): comandos dev-only (p.ej. `seed_demo_cards`) NO se
        // listan en builds de producción. El backend además los rechaza en release (defensa en
        // capas), pero ni siquiera deben aparecer/dispararse desde esta palette universal en prod.
        // Vite expone `import.meta.env.DEV`; el cast evita el error de tsc (ImportMeta sin `env`) y
        // en Node/tests cae a `false` — mismo patrón que `isDevBuild` en Shell.tsx (REFORMA 5).
        const devVisible = Boolean(
          (import.meta as unknown as { env?: { DEV?: boolean } }).env?.DEV,
        );
        const usable = usablePaletteCommands(cmds, devVisible);
        const realIds = new Set(usable.map((c) => c.id));
        setAll([...usable, ...navCommandDefs().filter((n) => !realIds.has(n.id))]);
        setLoading(false);
      })
      .catch((e) => {
        if (!alive) return;
        setError(
          typeof e === "string" ? e : (e as Error)?.message ?? "no se pudo cargar el registry",
        );
        setLoading(false);
      });
    return () => {
      alive = false;
    };
  }, []);

  // Foco al search al abrir.
  useEffect(() => {
    inputRef.current?.focus();
  }, []);

  // 047 FR-001 — map id→run de las acciones contextuales (executeNow lo consulta antes del invoke).
  const ctxRun = useMemo(() => {
    const m = new Map<string, () => void>();
    for (const a of contextActions ?? []) m.set(a.id, a.run);
    return m;
  }, [contextActions]);

  // 047 FR-001 — índice completo: comandos del registry + nav sintéticos (en `all`) + acciones
  // contextuales de la vista activa (sintéticas, ejecutan `run`). Las contextuales primero para que,
  // con el search vacío, queden arriba (el sort estable por score=0 preserva este orden de inserción).
  const indexed = useMemo(
    () => [...contextCommandDefs(contextActions ?? []), ...all],
    [contextActions, all],
  );

  // Dominios disponibles (derivados del índice). "all" + categorías únicas.
  const domains = useMemo(() => {
    const set = new Set(indexed.map((c) => c.category).filter(Boolean));
    return ["all", ...Array.from(set).sort()];
  }, [indexed]);

  // ── Pipeline de filtrado + fuzzy ranking. ───────────────────────────────
  const results = useMemo(() => {
    const visible = indexed.filter((c) => {
      // Audit codex US2: `hidden` = oculto de TODA superficie (contrato Rust). NUNCA se
      // muestra, ni con `advanced`. El toggle advanced sólo revela `internal`.
      if (c.visibility === "hidden") return false;
      if (!showAdvanced && c.visibility !== "primary" && c.visibility !== "palette") return false;
      if (domain !== "all" && c.category !== domain) return false;
      if (scope !== "all" && c.scope !== scope) return false;
      if (risk !== "all" && c.risk !== risk) return false;
      return true;
    });
    const scored: { cmd: CommandDef; score: number }[] = [];
    for (const c of visible) {
      const s = commandScore(query, c);
      if (s != null) scored.push({ cmd: c, score: s });
    }
    // 047 FR-001 (audit-3 MED, codex) — prioridad EXPLÍCITA de las acciones contextuales: con scores
    // empatados (query vacío → score 0 para todas), el desempate por `localeCompare` NO preservaría el
    // orden de inserción, así que las contextuales no quedarían arriba. Las priorizamos por categoría
    // (`context` primero) ANTES del desempate alfabético. Con query no-vacío manda el score (relevancia).
    const ctxRank = (c: CommandDef) => (c.category === "context" ? 0 : 1);
    scored.sort((a, b) => {
      if (b.score !== a.score) return b.score - a.score;
      const r = ctxRank(a.cmd) - ctxRank(b.cmd);
      if (r !== 0) return r;
      return a.cmd.label.localeCompare(b.cmd.label);
    });
    return scored.map((x) => x.cmd);
  }, [indexed, showAdvanced, domain, scope, risk, query]);

  // Reset selección cuando cambia el set de resultados.
  useEffect(() => {
    setActive(0);
  }, [query, domain, scope, risk, showAdvanced]);

  // Mantener visible la fila activa.
  useEffect(() => {
    const el = listRef.current?.querySelector<HTMLElement>(`[data-row="${active}"]`);
    el?.scrollIntoView({ block: "nearest" });
  }, [active]);

  function shortcutKeys(c: CommandDef): string[] | undefined {
    if (!c.shortcut) return undefined;
    return c.shortcut.split(/[\s+]+/).filter(Boolean);
  }

  // Ejecutar un comando: navegar (deeplink) o invocar el comando Tauri.
  // 015 T015 — invocar un comando con el `invoke` ENVUELTO: si el backend lo gatea (Destructive/
  // Credential), el wrapper dispara el modal GLOBAL de aprobación y, si se aprueba, re-invoca +
  // consume + ejecuta de forma transparente. Acá sólo resolvemos/cerramos o mostramos el error.
  async function executeNow(c: CommandDef) {
    setRunning(true);
    setRunError(null);
    try {
      // 047 FR-001 — acción contextual: ejecuta su `run` (abrir modal / navegar) y cierra.
      const ctx = ctxRun.get(c.id);
      if (ctx) {
        ctx();
        onClose();
        return;
      }
      if (c.deeplink && onNavigate) {
        onNavigate(c.deeplink);
        onClose();
        return;
      }
      await invoke(c.id);
      onClose();
    } catch (e) {
      setRunError(typeof e === "string" ? e : (e as Error)?.message ?? "falló la ejecución");
      setRunning(false);
    }
  }

  // Selección de una fila: invocar. El gate del backend + el modal global deciden la aprobación.
  function select(c: CommandDef) {
    setRunError(null);
    void executeNow(c);
  }

  function onKeyDown(e: React.KeyboardEvent) {
    // Si el modal global de aprobación está abierto, atrapa el foco/teclado por su cuenta.
    if (e.key === "ArrowDown") {
      e.preventDefault();
      setActive((i) => Math.min(i + 1, Math.max(results.length - 1, 0)));
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      setActive((i) => Math.max(i - 1, 0));
    } else if (e.key === "Enter") {
      e.preventDefault();
      const c = results[active];
      if (c) select(c);
    }
  }

  const subtitle = loading
    ? "cargando registry…"
    : error
      ? "error"
      : `${results.length} / ${indexed.length} comandos`;

  return (
    <ModalFrame
      title="Comandos"
      subtitle={subtitle}
      onClose={onClose}
      maxWidth={680}
      error={error}
      loading={loading}
      initialFocusRef={inputRef}
    >
      <div className="fxc-cp015" onKeyDown={onKeyDown}>
        {/* Search */}
        <input
          ref={inputRef}
          className="fxc-cp015__search"
          type="text"
          placeholder="Buscar comandos…"
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          autoComplete="off"
          spellCheck={false}
          aria-label="Buscar comandos"
        />

        {/* Filtros */}
        <div className="fxc-cp015__filters">
          <select
            className="fxc-cp015__select"
            value={domain}
            onChange={(e) => setDomain(e.target.value)}
            aria-label="Filtrar por dominio"
          >
            {domains.map((d) => (
              <option key={d} value={d}>
                {d === "all" ? "all domains" : d}
              </option>
            ))}
          </select>
          <select
            className="fxc-cp015__select"
            value={scope}
            onChange={(e) => setScope(e.target.value as CommandScope | "all")}
            aria-label="Filtrar por scope"
          >
            {SCOPE_FILTERS.map((s) => (
              <option key={s.value} value={s.value}>
                {s.label}
              </option>
            ))}
          </select>
          <select
            className="fxc-cp015__select"
            value={risk}
            onChange={(e) => setRisk(e.target.value as CommandRisk | "all")}
            aria-label="Filtrar por riesgo"
          >
            {RISK_FILTERS.map((r) => (
              <option key={r.value} value={r.value}>
                {r.label}
              </option>
            ))}
          </select>
          <label className="fxc-cp015__advanced">
            <input
              type="checkbox"
              checked={showAdvanced}
              onChange={(e) => setShowAdvanced(e.target.checked)}
            />
            advanced
          </label>
        </div>

        {/* Lista */}
        <div className="fxc-cp015__list" ref={listRef} role="listbox" aria-label="Comandos">
          {results.length === 0 ? (
            <div className="fxc-state" role="status">
              Sin comandos para «{query}».
            </div>
          ) : (
            results.map((c, i) => (
              <div key={c.id} data-row={i} role="option" aria-selected={i === active}>
                <CommandRow
                  label={c.label}
                  description={c.description || c.id}
                  icon={categoryGlyph(c.category)}
                  shortcut={shortcutKeys(c)}
                  risk={c.risk}
                  active={i === active}
                  onSelect={() => {
                    setActive(i);
                    select(c);
                  }}
                />
              </div>
            ))
          )}
        </div>

        {runError && (
          <div className="fxc-state fxc-state--error" role="alert">
            {runError}
          </div>
        )}
      </div>
    </ModalFrame>
  );
}
