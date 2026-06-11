// 018 Fase 2 US2 (T025) — windowManager: resolución label↔window_key + invariante de mapeo.
// Estilo node:assert (sin DOM, sin Tauri) — sólo la lógica PURA (resolveWindowLabel /
// isValidWindowKey / isMainWindow). El invoke real (detach/close/list) es backend, no se mockea
// acá; su corrección la cubren los tests Rust (window_registry / window_reattach).
import assert from "node:assert/strict";
import {
  resolveWindowLabel,
  isValidWindowKey,
  isMainWindow,
  windowFor,
} from "../windowManager.ts";
import { MAIN_WINDOW_KEY } from "../layoutConfig.ts";

let pass = 0;
let fail = 0;
const t = (name: string, fn: () => void) => {
  try {
    fn();
    pass++;
  } catch (e) {
    fail++;
    console.log(`FAIL ${name}: ${(e as Error).message}`);
    process.exitCode = 1;
  }
};

t("main window (no param) → main", () => {
  assert.equal(resolveWindowLabel(""), MAIN_WINDOW_KEY);
  assert.equal(resolveWindowLabel("?foo=bar"), MAIN_WINDOW_KEY);
});

t("detached param → that window_key", () => {
  assert.equal(resolveWindowLabel("?window_key=detached-1"), "detached-1");
  assert.equal(resolveWindowLabel("?window_key=detached-42"), "detached-42");
  // tolera el ? inicial o no.
  assert.equal(resolveWindowLabel("window_key=detached-3"), "detached-3");
});

t("malformed / spoofed window_key falls back to main", () => {
  // patrón inválido → main (defensa anti-URL-manipulada; el backend igual valida server-side).
  assert.equal(resolveWindowLabel("?window_key=../etc/passwd"), MAIN_WINDOW_KEY);
  assert.equal(resolveWindowLabel("?window_key=detached-0"), MAIN_WINDOW_KEY); // N debe ser ≥1
  assert.equal(resolveWindowLabel("?window_key=detached-"), MAIN_WINDOW_KEY);
  assert.equal(resolveWindowLabel("?window_key=evil"), MAIN_WINDOW_KEY);
  assert.equal(resolveWindowLabel("?window_key="), MAIN_WINDOW_KEY);
});

t("isValidWindowKey accepts main + detached-N, rejects rest", () => {
  assert.ok(isValidWindowKey("main"));
  assert.ok(isValidWindowKey("detached-1"));
  assert.ok(isValidWindowKey("detached-99"));
  assert.ok(!isValidWindowKey("detached-0"));
  assert.ok(!isValidWindowKey("detached-01")); // sin leading zero
  assert.ok(!isValidWindowKey("detached"));
  assert.ok(!isValidWindowKey("main-2"));
  assert.ok(!isValidWindowKey(""));
});

t("isMainWindow only for main label", () => {
  assert.ok(isMainWindow(MAIN_WINDOW_KEY));
  assert.ok(!isMainWindow("detached-1"));
});

// INVARIANTE de mapeo (FR-007): un panel_id se monta en UNA sola webview. Acá la modelamos a
// nivel de DATOS: dado un LayoutConfigV1 (movido por detach), cada panel_id aparece en EXACTAMENTE
// una ventana (el detach MUEVE el Leaf, no lo duplica). La unicidad la enforcea Rust (validate()),
// pero fijamos el contrato del lado que consume el árbol para renderizar.
import type { LayoutConfigV1, PanelLayoutNode } from "../layoutConfig.ts";

function panelIdsOf(node: PanelLayoutNode, out: string[]): void {
  if (node.node === "leaf") out.push(node.panel.panelId);
  else node.children.forEach((c) => panelIdsOf(c, out));
}

t("panel_id appears in exactly one window (no double-mount)", () => {
  // Modelo de un layout tras detach: m1 en main, m2 en detached-1. Ningún id repetido cross-window.
  const cfg: LayoutConfigV1 = {
    version: 1,
    workspaceId: "default",
    revision: 3,
    windows: [
      {
        windowKey: "main",
        kind: "main",
        layout: { node: "split", direction: "horizontal", children: [{ node: "leaf", panel: { panelType: "claude", panelId: "m1" } }] },
      },
      {
        windowKey: "detached-1",
        kind: "detached",
        layout: { node: "leaf", panel: { panelType: "codex", panelId: "m2" } },
      },
    ],
  };
  const all: string[] = [];
  for (const w of cfg.windows) panelIdsOf(w.layout, all);
  const unique = new Set(all);
  assert.equal(all.length, unique.size, "ningún panel_id se monta en 2 ventanas");
  assert.deepEqual(all.sort(), ["m1", "m2"]);
});

// 018 US2 audit (#2): windowFor — una DETACHED cuyo window_key desapareció NUNCA cae a Main.
t("windowFor: detached con window_key ausente → null (NO fallback a Main)", () => {
  const cfg: LayoutConfigV1 = {
    version: 1,
    workspaceId: "default",
    revision: 4,
    windows: [
      {
        windowKey: "main",
        kind: "main",
        layout: { node: "leaf", panel: { panelType: "claude", panelId: "m1" } },
      },
    ],
  };
  // Main resuelve a su propio árbol.
  assert.deepEqual(windowFor(cfg, "main"), cfg.windows[0].layout);
  // detached-1 ya NO existe en la config (reatado durante su cierre) → null, NO el árbol de Main.
  assert.equal(windowFor(cfg, "detached-1"), null, "detached ausente no debe montar el árbol de Main");
});

t("windowFor: detached presente → su propio subárbol", () => {
  const cfg: LayoutConfigV1 = {
    version: 1,
    workspaceId: "default",
    revision: 5,
    windows: [
      { windowKey: "main", kind: "main", layout: { node: "leaf", panel: { panelType: "claude", panelId: "m1" } } },
      { windowKey: "detached-2", kind: "detached", layout: { node: "leaf", panel: { panelType: "codex", panelId: "m2" } } },
    ],
  };
  assert.deepEqual(windowFor(cfg, "detached-2"), cfg.windows[1].layout);
});

console.log(`windowManager: ${pass} passed, ${fail} failed`);
