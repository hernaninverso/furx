// 018 Fase 2 US1 (T015) — tests de los mutadores PUROS del árbol + migración (forma).
// Estilo node:assert. `node scripts/test-all.mjs`.
import assert from "node:assert/strict";
import {
  leaves,
  panelIds,
  findLeaf,
  replaceLeaf,
  splitLeaf,
  closeLeaf,
  groupAsTab,
  nodePathToLeaf,
} from "../layoutTree.ts";
import { emptyLayoutConfig, configFromRaw } from "../layoutConfig.ts";
import type { PanelLayoutNode, PanelDescriptor } from "../layoutConfig.ts";

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

const desc = (id: string, type = "terminal"): PanelDescriptor => ({ panelType: type, panelId: id, params: null });
const leaf = (id: string, type = "terminal"): PanelLayoutNode => ({ node: "leaf", panel: desc(id, type) });

const tree: PanelLayoutNode = {
  node: "split",
  direction: "horizontal",
  children: [
    leaf("a"),
    { node: "tabs", active: 0, children: [leaf("b"), leaf("c")] },
  ],
};

t("leaves + panelIds in DFS order", () => {
  assert.deepEqual(panelIds(tree), ["a", "b", "c"]);
  assert.equal(leaves(tree).length, 3);
});

t("findLeaf", () => {
  assert.equal(findLeaf(tree, "b")?.panelId, "b");
  assert.equal(findLeaf(tree, "zzz"), null);
});

t("replaceLeaf is pure (input unchanged)", () => {
  const before = JSON.stringify(tree);
  const next = replaceLeaf(tree, "a", leaf("a2"));
  assert.equal(JSON.stringify(tree), before, "input no mutado");
  assert.deepEqual(panelIds(next), ["a2", "b", "c"]);
});

t("splitLeaf creates a split with both panes", () => {
  const next = splitLeaf(tree, "a", "vertical", desc("a-new"));
  // 'a' ahora es un split[v](a, a-new).
  assert.deepEqual(panelIds(next), ["a", "a-new", "b", "c"]);
  // estructura: el primer hijo del root es ahora un split.
  if (next.node === "split") {
    assert.equal(next.children[0].node, "split");
    if (next.children[0].node === "split") assert.equal(next.children[0].direction, "vertical");
  }
});

t("splitLeaf on unknown id → no change", () => {
  const next = splitLeaf(tree, "nope", "vertical", desc("x"));
  assert.deepEqual(panelIds(next), ["a", "b", "c"]);
});

t("closeLeaf removes leaf and collapses unary split", () => {
  // cerrar 'a' deja split[h]( tabs(b,c) ) → colapsa a tabs(b,c).
  const next = closeLeaf(tree, "a");
  assert.ok(next !== null);
  assert.equal(next!.node, "tabs");
  assert.deepEqual(panelIds(next!), ["b", "c"]);
});

t("closeLeaf inside tabs keeps split", () => {
  // cerrar 'b' deja split[h]( a, tabs(c) ) → tabs(c) sigue siendo tabs de 1 (no colapsa tabs).
  const next = closeLeaf(tree, "b");
  assert.ok(next !== null);
  assert.equal(next!.node, "split");
  assert.deepEqual(panelIds(next!), ["a", "c"]);
});

t("closeLeaf last leaf → null (empty)", () => {
  assert.equal(closeLeaf(leaf("only"), "only"), null);
});

t("closeLeaf never reports killing a process (pure tree op)", () => {
  // El helper sólo transforma el árbol; no toca procesos (no hay efecto). Verificamos
  // que cerrar una hoja NO elimina las demás (los otros panes/procesos siguen en el árbol).
  const next = closeLeaf(tree, "a");
  assert.deepEqual(panelIds(next!), ["b", "c"], "los demás panes permanecen");
});

t("groupAsTab wraps leaf in tabs", () => {
  const next = groupAsTab(tree, "a", desc("a-tab"));
  // 'a' ahora es tabs(a, a-tab) con a-tab activo.
  assert.deepEqual(panelIds(next), ["a", "a-tab", "b", "c"]);
});

t("nodePathToLeaf gives stable indices", () => {
  assert.deepEqual(nodePathToLeaf(tree, "a"), [0]);
  assert.deepEqual(nodePathToLeaf(tree, "c"), [1, 1]);
  assert.equal(nodePathToLeaf(tree, "nope"), null);
});

t("migration shape: legacy panes → v1 single Main split (via configFromRaw)", () => {
  // Espejo de migrate_v0_to_v1 (Rust): el raw que el backend devuelve tras migrar un
  // layout legacy es un LayoutConfigV1 con 1 Main + un split horizontal de N hojas.
  const raw = {
    version: 1,
    workspace_id: "default",
    revision: 0,
    windows: [
      {
        window_key: "main",
        kind: "main" as const,
        layout: {
          node: "split" as const,
          direction: "horizontal" as const,
          children: [
            { node: "leaf" as const, panel: { panel_type: "claude-A", panel_id: "p1", params: { title: "P1" } } },
            { node: "leaf" as const, panel: { panel_type: "codex", panel_id: "p2", params: null } },
          ],
        },
      },
    ],
  };
  const cfg = configFromRaw(raw);
  assert.equal(cfg.windows.length, 1);
  assert.equal(cfg.windows[0].kind, "main");
  assert.deepEqual(panelIds(cfg.windows[0].layout), ["p1", "p2"]);
  // panel_type → panelType (CLASE), panel_id → panelId (INSTANCIA).
  const p1 = findLeaf(cfg.windows[0].layout, "p1");
  assert.equal(p1?.panelType, "claude-A");
});

t("empty workspace migrates to a valid single Main empty split", () => {
  const cfg = emptyLayoutConfig("ws-x");
  assert.equal(cfg.windows.length, 1);
  assert.equal(cfg.windows[0].kind, "main");
  assert.equal(panelIds(cfg.windows[0].layout).length, 0);
});

console.log(`layoutTree: ${pass} passed, ${fail} failed`);
