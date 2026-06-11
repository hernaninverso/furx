import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Shell } from "./Shell";
// 015 T021 — error boundary GLOBAL: un crash de la UI muestra un fallback, no pantalla en blanco.
import { ErrorBoundary } from "./components/ErrorBoundary";
// 016 US1 — boundary i18n: provee `t()`/idioma a todo el árbol. Va SIEMPRE ON (no flag): degrada al
// texto fuente ante miss, así nada se rompe (FR-003). Anti-FOUC ya resolvió el idioma en index.html.
import { I18nProvider } from "./lib/i18n";
// 018 Fase 2 US2 — una webview DETACHED (`?window_key=detached-N`) NO renderiza el Shell entero,
// sino sólo el subárbol de SU ventana (DetachedWindow). La Main (sin el param) sigue con Shell.
import { DetachedWindow } from "./components/DetachedWindow";
import { resolveWindowLabel } from "./lib/windowManager";
import { MAIN_WINDOW_KEY } from "./lib/layoutConfig";

interface HealthInfo {
  version: string;
  db_ok: boolean;
}

export function App() {
  const [health, setHealth] = useState<HealthInfo | null>(null);
  const [err, setErr] = useState<string | null>(null);
  // Resuelto UNA vez al montar: ¿somos la ventana Main o una detached? (estable por webview).
  const [windowLabel] = useState<string>(() =>
    resolveWindowLabel(typeof window !== "undefined" ? window.location.search : ""),
  );
  const isDetached = windowLabel !== MAIN_WINDOW_KEY;

  useEffect(() => {
    invoke<HealthInfo>("health")
      .then(setHealth)
      .catch((e) => setErr(String(e)));
  }, []);

  if (err) {
    return <div style={{ padding: 30, color: "var(--red)" }}>Boot failed: {err}</div>;
  }
  if (!health) {
    return <div style={{ padding: 30, color: "#6c7b91" }}>Booting Furx…</div>;
  }
  // Ventana DETACHED: sólo su subárbol (sin chrome de la app). BYOK intacto (gate central deniega
  // comandos de credencial off-Main). Tema V3 dark+light + anti-FOUC ya resueltos en index.html.
  if (isDetached) {
    return (
      <I18nProvider>
        <ErrorBoundary scope="global">
          <DetachedWindow />
        </ErrorBoundary>
      </I18nProvider>
    );
  }
  return (
    <I18nProvider>
      <ErrorBoundary scope="global">
        <Shell version={health.version} />
      </ErrorBoundary>
    </I18nProvider>
  );
}
