import { useEffect, useRef } from "react";
import { Terminal as XTerm } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import { WebglAddon } from "@xterm/addon-webgl";
import { invoke } from "@tauri-apps/api/core";
import { listen, UnlistenFn } from "@tauri-apps/api/event";
import "@xterm/xterm/css/xterm.css";

interface Props {
  paneId: string;
  mode: string;
  cwd?: string;
  /** 006 — si está, el backend resuelve el runtime desde este agente (override de mode). */
  agentProfileId?: string;
  /** 008 — override del nombre de la sesión tmux (orquestación: única por tarea). */
  sessionOverride?: string;
  /** F9 — tap into PTY output so the shell can run heuristic suggestions. */
  onOutput?: (data: string) => void;
  /** 018 Fase 2 HIGH-1 (audit) — binding del lease: cuando el Terminal vive dentro del
   *  WorkspaceView (flag newWorkspace ON), su `pty_write` declara (windowLabel,
   *  mountInstanceId) del attach de ESTE montaje → pasa el guard fail-closed. En el path
   *  legacy (PanesView, flag OFF) quedan `undefined` → fail-open (no se manda nada). */
  leaseWindowLabel?: string;
  leaseMountInstanceId?: string;
}

// V3 "atelier" terminal palettes. xterm takes a JS theme object (it can't read
// CSS vars), so we ship two and swap on the `.dark` class. Dark = warm ink,
// light = warm paper; ANSI tones tuned to read on each background.
const XTERM_DARK = {
  background: "#16130f", foreground: "#efe7d8", cursor: "#ff8a6e", cursorAccent: "#16130f",
  selectionBackground: "rgba(255,138,110,0.28)",
  black: "#16130f", red: "#f0816f", green: "#7fc98a", yellow: "#e8b765", blue: "#6b9fd0",
  magenta: "#d0a0e0", cyan: "#56c8c8", white: "#efe7d8",
  brightBlack: "#79705f", brightRed: "#f59e8e", brightGreen: "#9bd9a4", brightYellow: "#f0cd86",
  brightBlue: "#8fbce0", brightMagenta: "#e0bced", brightCyan: "#6fe0d8", brightWhite: "#fffaf0",
} as const;
const XTERM_LIGHT = {
  background: "#faf7f0", foreground: "#1c1814", cursor: "#bf3f18", cursorAccent: "#faf7f0",
  selectionBackground: "rgba(255,92,53,0.20)",
  black: "#1c1814", red: "#a8412c", green: "#3a6b3f", yellow: "#9a6011", blue: "#2f6aa0",
  magenta: "#8a4fb0", cyan: "#0a8f86", white: "#5a5246",
  brightBlack: "#635849", brightRed: "#8f3322", brightGreen: "#2f5733", brightYellow: "#7d4d0d",
  brightBlue: "#26568a", brightMagenta: "#723f96", brightCyan: "#0a6878", brightWhite: "#1c1814",
} as const;
const isDarkTheme = () =>
  typeof document !== "undefined" && document.documentElement.classList.contains("dark");
const xtermTheme = (dark: boolean) => ({ ...(dark ? XTERM_DARK : XTERM_LIGHT) });

// Spawn + wire xterm <-> backend PTY. Lifetime is tied to the React mount.
export function Terminal({ paneId, mode, cwd, agentProfileId, sessionOverride, onOutput, leaseWindowLabel, leaseMountInstanceId }: Props) {
  const containerRef = useRef<HTMLDivElement>(null);
  const termRef = useRef<XTerm | null>(null);
  // 056 scroll-fix — sesión tmux PER-PANE. Por default `FURX_<paneId>` (NO `FURX_<mode>`): dos panes
  // del MISMO programa (dos `zsh`, dos `claude`) ya NO comparten la sesión tmux — antes attacheaban a
  // la misma `FURX_<mode>` con `-A` → era la MISMA sesión mostrada dos veces (scrolleabas una y se movía
  // la otra). `sessionOverride` (orquestación) sigue ganando. `paneId` es estable entre remounts del
  // mismo pane → el resume per-pane funciona. capture-history + label + spawn usan la MISMA key: el
  // backend (`furx_session_name`) sanitiza TODO char no-[A-Za-z0-9_]→`_` igual en el override y en
  // capture (y el label de abajo replica esa sanitización). Se pierde "abrir otro zsh recupera el zsh
  // anterior" — deseable: ERA la causa del bug.
  const sessionKey = sessionOverride && sessionOverride.trim().length > 0 ? sessionOverride.trim() : paneId;

  useEffect(() => {
    const container = containerRef.current;
    if (!container) return;

    const term = new XTerm({
      // V3: single mono stack for the whole app (--mono token). xterm can't read
      // CSS vars in its config, so we replicate the exact token stack here.
      fontFamily: "'Space Mono', 'SF Mono', Menlo, Consolas, 'Liberation Mono', monospace",
      fontSize: 12,
      lineHeight: 1.25,
      cursorBlink: true,
      cursorStyle: "bar",
      allowProposedApi: true,
      // Bug-fix 2026-05-26: no podía scrollear arriba en sesiones previas.
      // Codex MED fix B9: bajado de 100k → 50k. 50k líneas × ~120 chars × 8 panes ≈ 48MB
      // worst-case (xterm comprime). Default era 1000 (insufficient para Claude verbose).
      scrollback: 50_000,
      // Scroll on user input only — al tipear vuelve abajo, pero NO se mueve si el user
      // está leyendo historia (scrollOnInput=false sería molesto al revés).
      scrollOnUserInput: true,
      // Mouse wheel scroll funciona via xterm's internal viewport — exponerlo para que el
      // contenedor padre no robe el event.
      scrollSensitivity: 3,
      fastScrollSensitivity: 5,
      smoothScrollDuration: 0,
      theme: xtermTheme(isDarkTheme()),
    });
    termRef.current = term;

    const fit = new FitAddon();
    term.loadAddon(fit);
    term.open(container);

    // WebGL renderer is fastest but can fail on some HW — silently fall back to DOM.
    try {
      const webgl = new WebglAddon();
      webgl.onContextLoss(() => webgl.dispose());
      term.loadAddon(webgl);
    } catch {
      /* canvas/dom fallback is fine */
    }

    // Crash-fix (47/50 crash logs): si el container tiene dimensiones 0 (pane oculto / layout aún sin
    // calcular / vista no-panes activa), el renderer de xterm NO se inicializa, y un `fit()`/`write()`
    // posterior dispara `syncScrollArea` → `this._renderer.value.dimensions` es undefined → throw no
    // capturado. `safeFit` sólo ajusta con dimensiones reales (el ResizeObserver re-fitea cuando el
    // pane obtiene tamaño); `safeWrite` envuelve el write — la data SÍ entra al buffer de xterm aunque
    // el refresh del scroll falle, y se renderiza cuando el pane se hace visible.
    const safeFit = () => {
      if (container.clientWidth > 0 && container.clientHeight > 0) {
        try { fit.fit(); } catch { /* mid-unmount / renderer not ready */ }
      }
    };
    const safeWrite = (data: string) => {
      try { termRef.current?.write(data); } catch { /* renderer not ready yet — buffered, shown on resize */ }
    };

    safeFit();

    // Spawn backend PTY.
    let unlistenData: UnlistenFn | null = null;
    let unlistenExit: UnlistenFn | null = null;
    let resizeObserver: ResizeObserver | null = null;
    let resizeTimer: number | null = null;
    let alive = true;

    (async () => {
      // Codex HIGH 2026-05-26: register listeners BEFORE pty_spawn so the
      // first bytes from the spawned PTY are never lost. History capture
      // runs in parallel before the spawn.
      const dataFn = await listen<{ pane_id: string; data: string }>("pty:data", (ev) => {
        if (!alive) return;
        if (ev.payload.pane_id === paneId && termRef.current) {
          safeWrite(ev.payload.data);
          if (onOutput) onOutput(ev.payload.data);
        }
      });
      if (!alive) { dataFn(); return; }
      unlistenData = dataFn;

      const exitFn = await listen<{ pane_id: string; code: number | null }>("pty:exit", (ev) => {
        if (!alive) return;
        if (ev.payload.pane_id === paneId && termRef.current) {
          safeWrite(`\r\n\x1b[2;33m[process exited code=${ev.payload.code ?? "?"}]\x1b[0m\r\n`);
        }
      });
      if (!alive) { exitFn(); return; }
      unlistenExit = exitFn;

      // RESTORE-FIX 2026-05-26 — capture pre-existing tmux scrollback BEFORE
      // spawn so it's written to xterm before any live PTY data shows up.
      // Empty string if tmux unavailable or session doesn't exist yet.
      let history = "";
      try {
        // 022 US8 — capturamos la sesión REAL del pane (perfil/override-derivada), no el mode crudo.
        history = await invoke<string>("pty_capture_history", { mode: sessionKey, lines: 3000 });
      } catch {
        /* skip silently */
      }

      if (!alive) return;

      if (history && termRef.current) {
        // Codex LOW: reset attrs around the replay so style state can't bleed.
        safeWrite("\x1b[0m");
        safeWrite(history);
        if (!history.endsWith("\n")) safeWrite("\r\n");
        // 058 (ultrareview fix): replicar EXACTO `furx_session_name` (todo char no-[A-Za-z0-9_]→`_`),
        // no sólo los guiones — sino el label mostraba un nombre que no matchea la sesión real (paneId
        // puede traer `.`/`@`) y confundía al correlacionar con `tmux ls`.
        safeWrite(
          `\x1b[0m\x1b[2;36m[resumed tmux session FURX_${sessionKey.replace(/[^A-Za-z0-9_]/g, "_")}]\x1b[0m\r\n`,
        );
      }

      try {
        await invoke("pty_spawn", {
          paneId,
          mode,
          cwd: cwd ?? null,
          rows: term.rows,
          cols: term.cols,
          agentProfileId: agentProfileId ?? null,
          sessionOverride: sessionKey, // 056 — per-pane tmux session (FURX_<paneId>), no compartida por mode
        });
      } catch (e) {
        // 015 T014 (US5, audit re-ronda — codex HIGH): NO llamamos pty_kill acá. El backend
        // `pty_spawn` YA limpia su propio fallo (mata la half-session + marca la fila `failed`).
        // Un pty_kill stale desde el front sería peligroso: ahora rutea por el registry, y si el
        // mismo paneId ya se remontó como un run NUEVO running, cancelaría/mataría ESE run.
        if (alive) {
          safeWrite(`\r\n\x1b[31mpty_spawn failed: ${String(e)}\x1b[0m\r\n`);
        }
        return;
      }

      // 015 T014 (US5): si el cleanup corrió durante el await del spawn (el componente se
      // está desmontando), NO matamos el PTY — el proceso es propiedad del backend y SOBREVIVE
      // al unmount (está registrado + running en el process registry). Sólo soltamos la vista
      // (detach). Una nueva vista lo reatacha vía process_attach / process_list; el único modo
      // de terminarlo es una cancelación EXPLÍCITA (process_cancel).
      if (!alive) {
        return;
      }

      term.onData((data) => {
        // Idempotencia: cada keystroke lleva un (correlation_id, action_id) único
        // — el backend dedupea si recibe el mismo par dentro del TTL.
        // HIGH-1 (audit) + 018 US2 should-fix: el guard de lease usa el `window_label`
        // derivado SERVER-SIDE (anti-spoof) + el `mountInstanceId` que declara este montaje.
        // Si el Terminal vive en el WorkspaceView, declara su mount → pasa el guard fail-closed;
        // en el path legacy queda undefined → fail-open (sin lease, write normal).
        invoke("pty_write", {
          paneId,
          data,
          correlationId: paneId,
          actionId: `${Date.now()}-${Math.random().toString(36).slice(2, 8)}`,
          mountInstanceId: leaseMountInstanceId,
        }).catch((e) => { console.warn("pty_write failed", e); });
      });

      // Resize handling — observe the container, fit + propagate to PTY.
      // Perf-fix (Codex audit M9): debounce 80ms — al hacer drag del splitter se
      // disparaban decenas de invokes/sec.
      const lastDims = { rows: 0, cols: 0 };
      resizeObserver = new ResizeObserver(() => {
        if (resizeTimer !== null) window.clearTimeout(resizeTimer);
        resizeTimer = window.setTimeout(() => {
          try {
            safeFit();
            // Only invoke if dimensions actually changed
            if (term.rows !== lastDims.rows || term.cols !== lastDims.cols) {
              lastDims.rows = term.rows;
              lastDims.cols = term.cols;
              invoke("pty_resize", { paneId, rows: term.rows, cols: term.cols }).catch((e) => { console.warn("pty_resize failed", e); });
            }
          } catch {
            /* terminal may be mid-unmount */
          }
        }, 80);
      });
      resizeObserver.observe(container);
    })();

    // V3: live-swap the terminal palette when the app theme toggles.
    const themeObserver = new MutationObserver(() => {
      if (termRef.current) termRef.current.options.theme = xtermTheme(isDarkTheme());
    });
    themeObserver.observe(document.documentElement, { attributes: true, attributeFilter: ["class"] });

    return () => {
      alive = false;
      if (resizeTimer !== null) { window.clearTimeout(resizeTimer); resizeTimer = null; }
      themeObserver.disconnect();
      unlistenData?.();
      unlistenExit?.();
      resizeObserver?.disconnect();
      // 015 T014 (US5): DETACH, no kill. Desmontar el pane / cerrar la ventana NO mata el
      // proceso: vive en el backend (process registry) y sigue corriendo. Sólo descartamos la
      // vista local (xterm). Para terminarlo de verdad hace falta una cancelación EXPLÍCITA
      // (process_cancel / pty_kill ruteado por el registry). Re-montar reatacha vía
      // process_attach. Esto entrega la invariante "cerrar la ventana no mata el proceso".
      //
      // 058 (ultrareview A — crash-fix DE RAÍZ, diagnóstico codex): xterm 5.5.0 `Viewport` encola al
      // abrir un `syncScrollArea` por `setTimeout` NO-cancelable. Si desmontamos rápido (StrictMode
      // double-invoke en dev, o un remount veloz al restaurar el layout) y disponemos SINCRÓNICO, ese
      // timeout ya encolado dispara DESPUÉS del dispose → `_renderService.dimensions` lee
      // `this._renderer.value` (undefined tras dispose) → "undefined is not an object". Capturado por el
      // crash-logger (no-fatal) pero ensucia logs y dispara la alerta de Actividad. Fix: (1) ref
      // IDEMPOTENTE — sólo limpiar `termRef.current` si todavía apunta a ESTE term (no pisar un mount más
      // nuevo de StrictMode); (2) DIFERIR el dispose un macrotask — el `syncScrollArea` pendiente (encolado
      // en `open()`, antes que este setTimeout) corre con el renderer aún vivo, y recién después disponemos.
      if (termRef.current === term) {
        termRef.current = null;
      }
      setTimeout(() => {
        term.dispose();
      }, 0);
    };
    // 058 (ultrareview fix): `effectiveMode` SE QUITÓ de las deps (y de la key en Shell). Tras 056 el
    // `sessionKey` deriva de `sessionOverride || paneId` (NO de effectiveMode), y el spawn del backend
    // resuelve el runtime por `agentProfileId` contra su PROPIA db — NO depende del `agents` del front.
    // Cuando `agents` llega tarde y effectiveMode flippea legacy→synth, NI la sesión NI el comando real
    // cambian → re-ejecutar el efecto (dispose xterm + recaptura de 3000 líneas + re-spawn + banner)
    // era trabajo desperdiciado + un flash visible. Las deps reales (mode/cwd/agentProfileId/
    // sessionOverride/paneId) cubren todo remount legítimo. eslint-disable-next-line react-hooks/exhaustive-deps
  }, [paneId, mode, cwd, agentProfileId, sessionOverride]);

  return <div ref={containerRef} className="terminal-host" />;
}
