// 022 US13 · L4 (e2e) — config WebdriverIO para `tauri-driver`. SCAFFOLDING (NO se ejecuta en macOS).
// ──────────────────────────────────────────────────────────────────────────
// Camino trazado, no ejecutado: este archivo corre SÓLO en CI Linux (ver e2e/README.md). Apple no
// provee WebDriver de escritorio → en macOS no hay sesión posible. NO usa Playwright: `tauri-driver`
// envuelve WebKitWebDriver (WebKitGTK) y expone una sesión W3C contra el WebView nativo de la app.
//
// Patrón oficial Tauri (https://v2.tauri.app/develop/tests/webdriver/wdio/):
//   - `onPrepare` spawnea `tauri-driver` (puerto 4444).
//   - `capabilities` apunta `tauri:options.application` al binario release.
//   - `onComplete` mata el driver.
import { spawn, spawnSync, type ChildProcess } from "node:child_process";
import { resolve, dirname } from "node:path";
import { fileURLToPath } from "node:url";

// ESM no tiene `__dirname`: derivarlo de import.meta.url (este config es .ts/ESM, corre con tsx en CI).
const __dirname = dirname(fileURLToPath(import.meta.url));

// Binario release de la app (lo genera `npm run tauri build` en Linux).
const APP_BINARY = resolve(__dirname, "..", "src-tauri", "target", "release", "furx");

let tauriDriver: ChildProcess | undefined;

export const config: WebdriverIO.Config = {
  runner: "local",
  specs: ["./specs/**/*.e2e.ts"],
  maxInstances: 1, // app de escritorio: una sola sesión a la vez.

  capabilities: [
    {
      // `tauri:options` lo consume tauri-driver para lanzar el binario.
      "tauri:options": { application: APP_BINARY },
      // El navegador subyacente es WebKitGTK (vía WebKitWebDriver).
      browserName: "wry",
    } as WebdriverIO.Capabilities,
  ],

  // tauri-driver escucha en 4444 (proxy al WebKitWebDriver).
  hostname: "127.0.0.1",
  port: 4444,
  path: "/",

  logLevel: "info",
  framework: "mocha",
  mochaOpts: { ui: "bdd", timeout: 60_000 },
  reporters: ["spec"],

  // Asegura el binario builded antes de la sesión (en CI: `npm run tauri build`).
  onPrepare: () => {
    spawnSync("cargo", ["build", "--release"], { cwd: resolve(__dirname, "..", "src-tauri"), stdio: "inherit" });
  },

  // Arranca tauri-driver justo antes de la sesión.
  beforeSession: () => {
    tauriDriver = spawn("tauri-driver", [], { stdio: [null, process.stdout, process.stderr] });
  },

  // Mata tauri-driver al terminar.
  afterSession: () => {
    tauriDriver?.kill();
  },
};
