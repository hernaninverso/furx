# L4 — E2E con `tauri-driver` (CI Linux, NO Playwright)

> Spec 022 · US13 / FR-018 — nivel 4 del gate de testing.

Este directorio es el **scaffolding** del e2e de nivel 4. **NO corre en macOS**: Apple no
provee un WebDriver para apps de escritorio (no existe `WKWebView` driver para Tauri en macOS).
El e2e real corre en **CI Linux** con [`tauri-driver`](https://v2.tauri.app/develop/tests/webdriver/)
(que envuelve `WebKitWebDriver` de WebKitGTK) + [WebdriverIO](https://webdriver.io/). **NO usamos
Playwright** (Playwright no habla con el WebView nativo de Tauri; controla Chromium/Firefox/WebKit
de navegador, no la ventana de la app empaquetada).

## Por qué tauri-driver y no Playwright

- El binario de Furx renderiza en el **WebView del sistema** (WebKitGTK en Linux, WKWebView en
  macOS, WebView2 en Windows), no en un navegador. Playwright no lo controla.
- `tauri-driver` expone una sesión WebDriver W3C contra ese WebView → WebdriverIO/Selenium lo
  manejan como cualquier sesión remota.
- Soporte oficial Tauri: **solo Linux** hoy (WebKitWebDriver). Por eso L4 vive en CI Linux.

## Qué se necesita en CI Linux (Ubuntu)

```bash
# Dependencias del sistema (WebKitGTK + driver):
sudo apt-get update
sudo apt-get install -y webkit2gtk-driver xvfb \
  libwebkit2gtk-4.1-dev libgtk-3-dev librsvg2-dev

# tauri-driver (cargo):
cargo install tauri-driver --locked

# Build de la app en modo release (genera el binario que el driver lanza):
npm ci
npm run tauri build      # produce src-tauri/target/release/furx

# Correr el e2e bajo un display virtual:
xvfb-run -a npm run test:e2e
```

## Archivos

- `wdio.conf.ts` — config WebdriverIO: arranca/mata `tauri-driver`, apunta al binario
  `src-tauri/target/release/furx`, y registra los specs.
- `specs/smoke.e2e.ts` — recorrido smoke del MVP P0 (la app levanta, hay panes, la nav existe).
  Es el camino trazado; se completa cuando CI Linux esté habilitado.
- `.github/workflows/e2e-linux.yml.example` (en la raíz `docs/`) — workflow de referencia.

## Comando

`npm run test:e2e` (definido en `package.json`, **solo funciona en Linux con las deps de arriba**;
en macOS sale con un mensaje claro, no ejecuta nada).

## Dependencias para CI Linux (NO instaladas en el repo — L4 es scaffolding)

El L4 e2e corre SÓLO en CI Linux. Antes de `npm run test:e2e` el job de CI debe instalar
las dev-deps de WebdriverIO + el driver de Tauri (no van en `package.json` del repo para no
inflar el install de Mac/dev):

```bash
# en el runner Linux del CI:
npm i -D @wdio/cli @wdio/local-runner @wdio/mocha-framework webdriverio tsx
cargo install tauri-driver --locked   # WebKitWebDriver wrapper
sudo apt-get install -y webkit2gtk-driver xvfb
npm run tauri build                    # genera el binario release
xvfb-run npm run test:e2e              # corre wdio.conf.ts contra el WebView nativo
```

`scripts/e2e-guard.mjs` es no-op en macOS (exit 0) y delega a `npx wdio run e2e/wdio.conf.ts` en Linux.
