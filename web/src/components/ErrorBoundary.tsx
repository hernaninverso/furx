// web/src/components/ErrorBoundary.tsx — 015 T021 (FR-014) · error boundary reusable.
//
// Class component propio (React 19 sigue necesitando class boundaries; evitamos la dep
// react-error-boundary). Dos scopes:
//   - "global": envuelve toda la app (en App.tsx). Un crash muestra un fallback con Reintentar /
//     Recargar app, en vez de pantalla en blanco.
//   - "panel": envuelve la vista activa (en Shell, key={view}). Un crash en UNA vista muestra un
//     fallback local ("Recargar panel") sin tumbar el resto de la app.
// El estado de tareas del BACKEND (PTYs, orquestación) sobrevive: T014 lo hace dueño del backend;
// el boundary sólo re-monta la UI (remount por `resetSeq`), no reinicia procesos.

import { Component, ErrorInfo, Fragment, ReactNode } from "react";

interface Props {
  scope: "global" | "panel";
  /// nombre de la vista/panel (para el mensaje + el log).
  name?: string;
  children: ReactNode;
}

interface State {
  error: Error | null;
  /// bump → re-monta los children (limpia el subtree que crasheó).
  resetSeq: number;
}

export class ErrorBoundary extends Component<Props, State> {
  state: State = { error: null, resetSeq: 0 };

  static getDerivedStateFromError(error: Error): Partial<State> {
    return { error };
  }

  componentDidCatch(error: Error, info: ErrorInfo) {
    // Log a la consola (la captura la consola de Tauri / devtools) — obligatorio para debug en prod.
    const tag = `${this.props.scope}${this.props.name ? `:${this.props.name}` : ""}`;
    // eslint-disable-next-line no-console
    console.error(`[furx] ErrorBoundary(${tag}):`, error, info.componentStack);
  }

  private reset = () => this.setState((s) => ({ error: null, resetSeq: s.resetSeq + 1 }));

  render() {
    const { error, resetSeq } = this.state;
    if (error) {
      const isGlobal = this.props.scope === "global";
      return (
        <div className={`error-boundary error-boundary--${this.props.scope}`} role="alert">
          <div className="eb-title">
            {isGlobal ? "Furx encontró un error" : `Error en ${this.props.name ?? "este panel"}`}
          </div>
          <div className="eb-msg">{error.message || String(error)}</div>
          <div className="eb-actions">
            <button type="button" className="fxc-btn" onClick={this.reset}>
              {isGlobal ? "Reintentar" : "Recargar panel"}
            </button>
            {isGlobal && typeof location !== "undefined" && (
              <button type="button" className="fxc-btn" onClick={() => location.reload()}>
                Recargar app
              </button>
            )}
          </div>
        </div>
      );
    }
    // `key={resetSeq}` sobre un Fragment: al reintentar, React re-monta el subtree → limpia lo que
    // había crasheado. Fragment (no un div display:contents) → CERO impacto de layout/estilos.
    return <Fragment key={resetSeq}>{this.props.children}</Fragment>;
  }
}
