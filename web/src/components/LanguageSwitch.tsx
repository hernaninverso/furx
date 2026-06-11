// web/src/components/LanguageSwitch.tsx — 016 US1 (T013) · selector de idioma para Settings.
//
// Cambiar el idioma re-renderiza la UI SIN reinicio (FR-005): `setLocale` del provider notifica al
// singleton i18n, el provider re-renderiza el árbol. Persiste vía i18n.ts (anti-FOUC al próximo boot).
// Estética V3: reusa `form-row` de Settings + tokens. Copy vía `t()`.

import { LOCALES, useI18n, type Locale } from "../lib/i18n";
import { trackEvent } from "../lib/telemetry";

export function LanguageSwitch() {
  const { locale, setLocale, t } = useI18n();
  return (
    <div className="form-row">
      <label htmlFor="furx-lang-select">{t("lang.label")}</label>
      <div className="form-input">
        <select
          id="furx-lang-select"
          value={locale}
          onChange={(e) => {
            const to = e.target.value as Locale;
            setLocale(to);
            // 016 US5 — telemetry opt-in: SÓLO el código de idioma destino (allowlisted). Gate interno.
            trackEvent("language_changed", { to });
          }}
          aria-label={t("lang.label")}
        >
          {LOCALES.map((l) => (
            <option key={l} value={l}>
              {/* 063 — etiqueta por la key lang.<code> (es/en/pt/it/fr/de). */}
              {t(`lang.${l}`)}
            </option>
          ))}
        </select>
      </div>
      <div className="hint muted">{t("lang.hint")}</div>
    </div>
  );
}
