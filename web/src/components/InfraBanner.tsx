// 042 FR-003 — banner discreto "inferencia no configurada". Presentacional puro: el Shell decide
// CUÁNDO mostrarlo (tras health-check del endpoint AIE) y le pasa `visible` + los callbacks.
// NO bloquea el uso de la app (la app funciona sin AIE, sólo sin las features de inferencia).

import { useT } from "../lib/i18n";

interface Props {
  visible: boolean;
  onOpenSettings: () => void;
  onDismiss: () => void;
}

export function InfraBanner({ visible, onOpenSettings, onDismiss }: Props) {
  const t = useT();
  if (!visible) return null;
  return (
    <div className="infra-banner" role="status" aria-live="polite">
      <span className="infra-banner-msg">
        {t("chrome.banner.infra")}{" "}
        <a
          role="button"
          tabIndex={0}
          onClick={onOpenSettings}
          onKeyDown={(e) => {
            // Enter/Space activan el link; Space ademas preventDefault para no scrollear la pagina.
            if (e.key === "Enter" || e.key === " ") { e.preventDefault(); onOpenSettings(); }
          }}
        >
          {t("chrome.banner.infraLink")}
        </a>.
      </span>
      <button
        type="button"
        className="infra-banner-close"
        aria-label={t("chrome.banner.dismiss")}
        onClick={onDismiss}
      >
        ×
      </button>
    </div>
  );
}
