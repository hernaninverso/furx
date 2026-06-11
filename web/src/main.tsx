import React from "react";
import ReactDOM from "react-dom/client";
import { App } from "./App";
import { installCrashHandlers } from "./crashLog";
import "./styles.css";
// US8 (spec 015): capa semántica de tokens + CSS de componentes canónicos.
// var() en tokens.css resuelve contra las primitivas V3 de styles.css (lazy).
import "./styles/tokens.css";
import "./styles/canonical.css";
// 022 US9 — design-system <Button> (escala cerrada de variantes sobre tokens V3).
import "./styles/buttonComponent.css";

// C2 — JS error + unhandledrejection capture; relays to backend rotated log.
installCrashHandlers();

ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>
);
