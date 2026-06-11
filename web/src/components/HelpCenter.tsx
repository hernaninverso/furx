// web/src/components/HelpCenter.tsx — 016 US2 (T021/T022/T023) · Centro de ayuda buscable.
//
// Panel sobre `ModalFrame` canónico (hereda focus-trap, ESC, backdrop, scroll-lock, V3). El contenido
// se DERIVA del Command Registry + navGroups (lib/help.ts, SSOT). Búsqueda fuzzy + agrupación por
// dominio. "Ir/ejecutar":
//   - entrada con `deeplink` → navega vía `onNavigate` (router interno del Shell). FR-008.
//   - entrada con `commandId` → invoca con el `invoke` ENVUELTO → el gate universal del kernel pide
//     aprobación para Destructive/Credential (NO bypass). FR-009/FR-023.
// Apertura contextual: `contextSection` posiciona el filtro/scroll en esa sección. FR-008.
// Copy 100% vía `t()` (FR-010). Estética V3 (sólo tokens/clases canónicas).

import { useEffect, useMemo, useRef, useState } from "react";
import { ModalFrame } from "./canonical";
import { invoke } from "../lib/invoke";
import { loadCommandRegistry, type CommandDef } from "../lib/commandRegistry";
import { buildHelpIndex, searchHelp, groupByDomain } from "../lib/help";
import { useT } from "../lib/i18n";

export interface HelpCenterProps {
  /** Cerrar el Help (ESC, backdrop, ×). */
  onClose: () => void;
  /** Navegar un deeplink furx://… (lo cablea el Shell al router interno). */
  onNavigate?: (deeplink: string) => void;
  /** Sección contextual de apertura (dominio o id) — posiciona el scroll. FR-008. */
  contextSection?: string;
  /** Relanzar el tour de primeros pasos desde Help (FR-014). Opcional. */
  onRelaunchTour?: () => void;
}

export function HelpCenter({ onClose, onNavigate, contextSection, onRelaunchTour }: HelpCenterProps) {
  const t = useT();
  const [cmds, setCmds] = useState<CommandDef[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [query, setQuery] = useState("");
  const [runError, setRunError] = useState<string | null>(null);
  const inputRef = useRef<HTMLInputElement>(null);
  const bodyRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    let alive = true;
    loadCommandRegistry()
      .then((c) => { if (alive) { setCmds(c); setLoading(false); } })
      .catch((e) => {
        if (!alive) return;
        // Fuera de Tauri o registry inaccesible: el Help degrada a SÓLO las entradas de navegación
        // (buildHelpIndex con [] sigue agregando los 6 dominios de navGroups). No es fatal.
        setError(typeof e === "string" ? e : (e as Error)?.message ?? "registry inaccesible");
        setCmds([]);
        setLoading(false);
      });
    return () => { alive = false; };
  }, []);

  // buildHelpIndex memoiza por identidad de `cmds` (council T075). useMemo refuerza por render.
  const index = useMemo(() => buildHelpIndex(cmds), [cmds]);
  const results = useMemo(() => searchHelp(index, query), [index, query]);
  const grouped = useMemo(() => groupByDomain(results), [results]);

  // Apertura contextual: precargar el query con la sección y hacer scroll a su grupo. FR-008.
  useEffect(() => {
    if (!contextSection || loading) return;
    const el = bodyRef.current?.querySelector<HTMLElement>(`[data-help-domain="${cssEscape(contextSection)}"]`);
    if (el) el.scrollIntoView({ block: "start", behavior: "smooth" });
  }, [contextSection, loading, grouped.length]);

  function activate(deeplink: string | null, commandId: string | null) {
    setRunError(null);
    if (deeplink) {
      onNavigate?.(deeplink);
      onClose();
      return;
    }
    if (commandId) {
      // Gate universal: si es Destructive/Credential, `invoke` dispara el modal global de aprobación.
      void invoke(commandId)
        .then(() => onClose())
        .catch((e) => setRunError(typeof e === "string" ? e : (e as Error)?.message ?? "falló la ejecución"));
    }
  }

  const subtitle = loading ? t("common.loading") : t("help.subtitle", { count: results.length });

  return (
    <ModalFrame
      title={t("help.title")}
      subtitle={subtitle}
      onClose={onClose}
      maxWidth={680}
      loading={loading}
      initialFocusRef={inputRef}
      footer={onRelaunchTour ? (
        <button type="button" className="fxc-btn" onClick={() => { onRelaunchTour(); onClose(); }}>
          {t("help.relaunchTour")}
        </button>
      ) : undefined}
    >
      <div className="fxc-help" ref={bodyRef}>
        <input
          ref={inputRef}
          className="fxc-cp015__search"
          type="text"
          placeholder={t("help.searchPlaceholder")}
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          autoComplete="off"
          spellCheck={false}
          aria-label={t("help.searchPlaceholder")}
        />
        {contextSection && (
          <div className="fxc-help__context muted">{t("help.contextHint", { section: contextSection })}</div>
        )}
        {error && (
          <div className="fxc-state fxc-state--error" role="alert">{error}</div>
        )}
        {results.length === 0 ? (
          <div className="fxc-state" role="status">{t("help.empty", { query })}</div>
        ) : (
          grouped.map((g) => (
            <section key={g.domain} className="fxc-help__group" data-help-domain={g.domain}>
              <h3 className="fxc-help__domain">{g.domain}</h3>
              <ul className="fxc-help__list">
                {g.entries.map((e) => (
                  <li key={e.id} className="fxc-help__entry">
                    <div className="fxc-help__entry-main">
                      <span className="fxc-help__entry-label">{e.label}</span>
                      <span className="fxc-help__entry-desc muted">
                        {e.description || t("help.noDescription")}
                      </span>
                    </div>
                    {(e.deeplink || e.commandId) && (
                      <button
                        type="button"
                        className="fxc-btn"
                        onClick={() => activate(e.deeplink, e.commandId)}
                        aria-label={`${t("help.openAction")} — ${e.label}`}
                      >
                        {t("help.openAction")}
                      </button>
                    )}
                  </li>
                ))}
              </ul>
            </section>
          ))
        )}
        {runError && (
          <div className="fxc-state fxc-state--error" role="alert">{runError}</div>
        )}
      </div>
    </ModalFrame>
  );
}

/// CSS.escape no existe en SSR/tests; fallback simple para el selector de atributo del scroll.
function cssEscape(s: string): string {
  if (typeof CSS !== "undefined" && typeof CSS.escape === "function") return CSS.escape(s);
  return s.replace(/["\\]/g, "\\$&");
}
