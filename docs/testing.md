# Gate de testing L1–L4 — Furx

> Spec 022 (madurez y coherencia de UI) · US13 / FR-018. El gate de testing sostiene los
> invariantes de las fases previas: cero literal de chrome fuera de `t()`, label de pane derivado
> de config, cada stat con destino de drill-down, design-system de botones, nav bien formada.

La pirámide tiene **4 niveles**. Cada nivel ataca un riesgo distinto y corre en un runner distinto;
los runners son **ortogonales** (uno no rompe al otro).

```
        ╱╲          L4  e2e        tauri-driver (CI Linux, NO Playwright)   ← camino trazado
       ╱──╲         L3  a11y       vitest-axe (axe-core) sobre el DOM
      ╱────╲        L2  componentes RTL + @tauri-apps/api/mocks (jsdom)
     ╱──────╲       L1  lógica     node --experimental-strip-types (puro)   ← base, 35 suites
    ╱────────╲
```

## L1 — Lógica pura (base)

- **Qué prueba**: lógica determinista sin DOM ni Tauri. Es el grueso del gate y lo más barato.
  Incluye los invariantes de "cero hardcode" del spec 022:
  - **nav bien formada** (`navGroups.test.ts`, `navWellFormed.test.ts`): IDs de grupo únicos,
    ningún grupo vacío, cada ítem con vista real del union `View` + ícono + label, ninguna vista
    huérfana ni duplicada, paridad i18n 1:1 (`nav.<id>` / `nav.item.<view>` en `es`+`en`).
  - **stats accionables** (`stats.test.ts`, `statsDrilldown.test.ts`): cada stat tiene un `destView`
    real **alcanzable por la nav**, el drill-down produce un `NavState` con el filtro one-shot
    correcto (incidents→open, monitors→down), valores derivados de datos (no hardcode),
    "Schema v3" eliminado, edge de valor 0 sigue navegable.
  - **paridad i18n runtime de la chrome de stats** (`chromeStatsI18nKeys.test.ts`): toda key
    `chrome.stats.*` que el código consume existe en `es` **y** `en` (cierra el hueco del
    translator inyectado, que `tsc` no cubre).
  - **chrome sin literales** (`chromeI18n.test.ts`, `sentenceCase.test.ts`): los literales migrados
    no reaparecen crudos; el catálogo base es sentence-case y sin "honesto/honest".
  - **shortcuts del registry** (`sidebarShortcuts.test.ts`): el sidebar deriva de `buildActions()`,
    nunca inventa literales.
- **Runner**: `scripts/test-all.mjs` — descubre `web/src/**/__tests__/*.test.ts` y los corre con el
  **type-stripping nativo de Node** (>= 23.6). Sin vitest/jest. Cada suite es autónoma (`process.exit`).
- **Comando**: `npm test`
- **Dónde corre**: cualquier OS con Node >= 23.6 (macOS o Linux).

## L2 — Componentes (RTL + mocks de Tauri)

- **Qué prueba**: el contrato VISIBLE de los componentes React: que renderizan el variant/estado
  correcto, que el click dispara el handler, y que un componente que **lee del backend** muestra el
  valor mockeado (no un placeholder). Usa los **mocks oficiales de Tauri** (`@tauri-apps/api/mocks`,
  `mockIPC`) para interceptar `invoke(...)` sin levantar la app.
  - `Button.test.tsx`: variant/size → clases canónicas, onClick, disabled/loading + a11y.
  - `backendMock.test.tsx`: patrón de referencia `mockIPC` (valor mockeado, estado vacío real, error
    fail-closed). Las vistas reales (PluginsView/IncidentInbox) reusan este patrón.
- **Runner**: **Vitest + jsdom** (`vitest.config.ts`). `include` matchea SÓLO `**/*.test.tsx`, así
  NO toca los `*.test.ts` de L1. `setupFiles` carga jest-dom + vitest-axe + `clearMocks()` entre tests.
- **Comando**: `npm run test:components`  (·  watch: `npm run test:watch`)
- **Dónde corre**: macOS o Linux. (jsdom emite un warning benigno de `HTMLCanvasElement.getContext`
  cuando axe intenta medir contraste — inocuo, no instalamos el paquete `canvas`.)

## L3 — Accesibilidad (axe)

- **Qué prueba**: baseline a11y con **axe-core** (`vitest-axe`) sobre el DOM renderizado: nombres
  accesibles de botones, dialogs con label, roles válidos, etc. `a11y.test.tsx` incluye un test-guard
  (`button-name`) que confirma que axe está REALMENTE activo (no un no-op que siempre pasa).
- **Runner**: el mismo Vitest de L2 (los matchers axe se cargan en el setup).
- **Comando**: `npm run test:a11y` (corre el archivo a11y; también va incluido en `test:components`).
- **Nota**: en jsdom NO se evalúa contraste de color (no computa estilos); las reglas estructurales
  (nombres, roles, ARIA) sí. El contraste se cubre en L4 (render real) y en revisión de diseño.

## L4 — E2E (tauri-driver, CI Linux — **NO Playwright**)

- **Qué prueba**: recorrido del MVP P0 sobre el **binario real** de la app (no el bundle JS): la app
  levanta, la chrome existe, los stats son accionables.
- **Por qué tauri-driver y no Playwright**: la app renderiza en el **WebView del sistema** (WebKitGTK
  en Linux), no en un navegador. Playwright controla Chromium/Firefox/WebKit de navegador, **no** la
  ventana nativa de Tauri. `tauri-driver` expone una sesión WebDriver W3C contra ese WebView;
  WebdriverIO la maneja.
- **Por qué SÓLO Linux / no en Mac**: Apple no provee WebDriver de escritorio para WKWebView;
  `tauri-driver` soporta hoy únicamente Linux (WebKitWebDriver). Por eso **L4 NO se ejecuta en macOS**
  — es el camino trazado, no corrido. En este repo el scaffolding existe y está documentado; el
  e2e real se habilita en CI Linux.
- **Scaffolding** (no ejecutado en Mac):
  - `e2e/wdio.conf.ts` — config WebdriverIO (spawnea/mata `tauri-driver`, apunta al binario release).
  - `e2e/specs/smoke.e2e.ts` — recorrido smoke del MVP P0.
  - `e2e/README.md` — deps de sistema + pasos.
  - `docs/e2e-linux.yml.example` — workflow GHA de referencia (copiar a `.github/workflows/`).
- **Comando**: `npm run test:e2e` — en macOS/Windows es un **no-op** con mensaje claro (exit 0);
  en Linux delega a `wdio run e2e/wdio.conf.ts` (requiere `webkit2gtk-driver` +
  `cargo install tauri-driver` + `npm run tauri build`).

## Resumen de comandos

| Nivel | Comando | Runner | Dónde |
|------|---------|--------|-------|
| L1 lógica | `npm test` | node type-strip (`scripts/test-all.mjs`) | macOS / Linux |
| L2 componentes | `npm run test:components` | Vitest + jsdom + RTL + mocks Tauri | macOS / Linux |
| L3 a11y | `npm run test:a11y` | Vitest + vitest-axe | macOS / Linux |
| L4 e2e | `npm run test:e2e` | tauri-driver + WebdriverIO | **solo CI Linux** |

## Reglas del gate (cómo NO romperlo)

- **L1 y L2/L3 son ortogonales**: L1 corre `*.test.ts` (sin JSX, node), L2/L3 corren `*.test.tsx`
  (JSX, vitest+jsdom). Un test de componente va a `*.test.tsx`; uno de lógica pura a `*.test.ts`.
  Nunca mezclar: un `*.test.ts` con JSX rompería el type-strip de Node, y un `*.test.tsx` no lo
  recoge el runner L1.
- **Dev-deps de L2/L3** se instalan con `npm install --save-dev --legacy-peer-deps` (conflictos de
  peer deps Vite+React).
- **`npm test` (L1) debe seguir verde** ante cualquier cambio: las capas nuevas son ADITIVAS.
- Cobertura tipo registry: cualquier vista/grupo/comando nuevo debe quedar cubierto por su test L1
  (igual que `registry_covers_all_handler_commands` en Rust).

## Estado actual (esta unidad)

- L1: **35 suites verdes** (32 base + 3 nuevas: `navWellFormed`, `statsDrilldown`,
  `chromeStatsI18nKeys`).
- L2: `Button.test.tsx` (6) + `backendMock.test.tsx` (3) — RTL + `mockIPC`.
- L3: `a11y.test.tsx` (4, incl. guard `button-name`) — vitest-axe.
- L4: scaffolding documentado (`e2e/`, `docs/e2e-linux.yml.example`), **no ejecutado en Mac**.
