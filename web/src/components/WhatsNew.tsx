// web/src/components/WhatsNew.tsx — 016 US3 (T032) · indicador NO-modal + panel de novedades.
//
// No-modal (FR-012, clarification): un PILL discreto arriba a la derecha cuando hay entradas nuevas;
// al hacer clic se abre un PANEL flotante (no bloquea la app, no atrapa el foco como un modal). Marcar
// visto persiste `lastSeen` y oculta el pill hasta la próxima versión (FR-013). Instalación fresca NO
// spamea: marca la versión actual como vista al montar (Edge "fresh install").
// Estética V3 (tokens). Copy chrome vía t(); el contenido de cada release es dato editorial.

import { useEffect, useMemo, useState } from "react";
import { resolveWhatsNew, setLastSeen, type WhatsNewState } from "../lib/whatsNew";
import { useT } from "../lib/i18n";

export interface WhatsNewProps {
  /** Versión instalada (Tauri health.version, vía Shell). */
  version: string;
  /** Panel abierto (controlado por el Shell — también se abre vía furx://whatsnew / ⌘K). */
  open: boolean;
  onOpen: () => void;
  onClose: () => void;
  /** Navegar el deeplink de una entrada (router interno del Shell). FR Acceptance 3. */
  onNavigate?: (deeplink: string) => void;
}

export function WhatsNew({ version, open, onOpen, onClose, onNavigate }: WhatsNewProps) {
  const t = useT();
  const [state, setState] = useState<WhatsNewState>(() => resolveWhatsNew(version));

  // Instalación fresca → marcar la versión actual como vista (sin spamear el historial). Edge case.
  useEffect(() => {
    const s = resolveWhatsNew(version);
    if (s.kind === "fresh") {
      setLastSeen(version);
      setState({ kind: "current", entries: [] });
    } else {
      setState(s);
    }
  }, [version]);

  const hasNew = state.kind === "upgrade" && state.entries.length > 0;

  // Marcar visto: persiste lastSeen y oculta el pill hasta la próxima versión.
  function markSeen() {
    setLastSeen(version);
    setState({ kind: "current", entries: [] });
    onClose();
  }

  // El pill sólo aparece si hay novedades nuevas (no-modal: no interrumpe). El panel puede abrirse
  // igual desde ⌘K aunque no haya nuevas (muestra "sin novedades").
  const entries = useMemo(() => state.entries, [state]);

  return (
    <>
      {hasNew && !open && (
        <button
          type="button"
          className="fxc-whatsnew-pill"
          onClick={onOpen}
          aria-label={t("whatsNew.pill")}
          title={t("whatsNew.title")}
        >
          ✦ {t("whatsNew.pill")}
          <span className="fxc-whatsnew-pill__count">{entries.length}</span>
        </button>
      )}
      {open && (
        <div className="fxc-whatsnew-panel" role="dialog" aria-label={t("whatsNew.title")}>
          <header className="fxc-whatsnew-panel__head">
            <div>
              <h2 className="fxc-whatsnew-panel__title">{t("whatsNew.title")}</h2>
              <div className="fxc-whatsnew-panel__sub muted">{t("whatsNew.subtitle")}</div>
            </div>
            <button type="button" className="modal-close-x" style={{ position: "static" }} onClick={onClose} aria-label={t("common.close")}>×</button>
          </header>
          <div className="fxc-whatsnew-panel__body">
            {entries.length === 0 ? (
              <div className="fxc-state" role="status">{t("whatsNew.empty")}</div>
            ) : (
              <ul className="fxc-whatsnew-list">
                {entries.map((n) => (
                  <li key={n.version} className="fxc-whatsnew-item">
                    <div className="fxc-whatsnew-item__ver">{t("whatsNew.version", { version: n.version })}</div>
                    <div className="fxc-whatsnew-item__title">{n.title}</div>
                    <div className="fxc-whatsnew-item__desc muted">{n.description}</div>
                    {n.deeplink && onNavigate && (
                      <button
                        type="button"
                        className="fxc-btn"
                        onClick={() => { onNavigate(n.deeplink!); onClose(); }}
                      >
                        {t("whatsNew.openFeature")}
                      </button>
                    )}
                  </li>
                ))}
              </ul>
            )}
          </div>
          <footer className="fxc-whatsnew-panel__foot">
            <button type="button" className="fxc-btn fxc-btn--primary" onClick={markSeen}>
              {t("whatsNew.markSeen")}
            </button>
          </footer>
        </div>
      )}
    </>
  );
}
