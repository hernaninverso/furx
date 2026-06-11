// 018 Fase 2 B1 (T067) — round-trip determinista LayoutConfigV1 ↔ dockview.
// Prueba que `node → toDockview(plan) → buildGridModel → fromDockview` reproduce la
// FORMA CANÓNICA del árbol (Split→branch, Tabs→stack, Leaf→panel) SIN persistir el
// JSON interno de dockview. Estilo node:assert (sin DOM, sin levantar dockview).
import assert from "node:assert/strict";
import {
  toDockview,
  fromDockview,
  buildGridModel,
  canonicalize,
} from "../dockviewAdapter.ts";
import type { PanelLayoutNode } from "../layoutConfig.ts";

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

const leaf = (id: string, type = "terminal"): PanelLayoutNode => ({
  node: "leaf",
  panel: { panelType: type, panelId: id, params: null },
});

/** El round-trip completo: node → plan → grid model → node, canonicalizado en ambos lados. */
function roundtrip(node: PanelLayoutNode): PanelLayoutNode {
  const plan = toDockview(node);
  const model = buildGridModel(plan);
  assert.ok(model !== null, "modelo no nulo");
  return canonicalize(fromDockview(model!));
}

t("single leaf round-trips", () => {
  const n = leaf("a");
  assert.deepEqual(roundtrip(n), canonicalize(n));
});

t("horizontal split of 2 leaves", () => {
  const n: PanelLayoutNode = {
    node: "split",
    direction: "horizontal",
    children: [leaf("a"), leaf("b")],
  };
  assert.deepEqual(roundtrip(n), canonicalize(n));
});

t("vertical split of 3 leaves", () => {
  const n: PanelLayoutNode = {
    node: "split",
    direction: "vertical",
    children: [leaf("a"), leaf("b"), leaf("c")],
  };
  assert.deepEqual(roundtrip(n), canonicalize(n));
});

t("tabs of 2 leaves → group stack", () => {
  const n: PanelLayoutNode = { node: "tabs", active: 0, children: [leaf("a"), leaf("b")] };
  const rt = roundtrip(n);
  assert.equal(rt.node, "tabs");
  if (rt.node === "tabs") {
    assert.equal(rt.children.length, 2);
    assert.deepEqual(
      rt.children.map((c) => (c.node === "leaf" ? c.panel.panelId : "?")),
      ["a", "b"],
    );
  }
});

t("nested: Split[h]( Leaf, Split[v](Leaf, Tabs(Leaf,Leaf)) )", () => {
  const n: PanelLayoutNode = {
    node: "split",
    direction: "horizontal",
    children: [
      leaf("a"),
      {
        node: "split",
        direction: "vertical",
        children: [leaf("b"), { node: "tabs", active: 1, children: [leaf("c"), leaf("d")] }],
      },
    ],
  };
  const rt = roundtrip(n);
  // La forma canónica preserva la estructura: panel_ids y tipos de nodo intactos.
  const ids: string[] = [];
  const collect = (x: PanelLayoutNode) => {
    if (x.node === "leaf") ids.push(x.panel.panelId);
    else x.children.forEach(collect);
  };
  collect(rt);
  assert.deepEqual(ids.sort(), ["a", "b", "c", "d"]);
  assert.deepEqual(rt, canonicalize(n), "round-trip == canónico");
});

t("toDockview plan: panel_id 1:1 con dockview", () => {
  const n: PanelLayoutNode = {
    node: "split",
    direction: "horizontal",
    children: [leaf("p1", "claude"), leaf("p2", "codex")],
  };
  const plan = toDockview(n);
  assert.equal(plan.length, 2);
  assert.equal(plan[0].panelId, "p1");
  assert.equal(plan[0].panelType, "claude");
  assert.equal(plan[0].position, undefined, "primer panel = raíz");
  assert.equal(plan[1].panelId, "p2");
  assert.deepEqual(plan[1].position, { type: "split", referencePanelId: "p1", direction: "right" });
});

t("toDockview tabs plan uses referenceGroupOf", () => {
  const n: PanelLayoutNode = { node: "tabs", active: 0, children: [leaf("p1"), leaf("p2")] };
  const plan = toDockview(n);
  assert.equal(plan[1].position?.type, "tab");
  if (plan[1].position?.type === "tab") {
    assert.equal(plan[1].position.referenceGroupOf, "p1");
  }
});

t("canonicalize flattens nested same-orientation splits", () => {
  const n: PanelLayoutNode = {
    node: "split",
    direction: "horizontal",
    children: [leaf("a"), { node: "split", direction: "horizontal", children: [leaf("b"), leaf("c")] }],
  };
  const c = canonicalize(n);
  assert.equal(c.node, "split");
  if (c.node === "split") {
    assert.equal(c.children.length, 3, "h(a, h(b,c)) → h(a,b,c)");
    assert.deepEqual(
      c.children.map((x) => (x.node === "leaf" ? x.panel.panelId : "?")),
      ["a", "b", "c"],
    );
  }
});

t("canonicalize collapses unary split", () => {
  const n: PanelLayoutNode = { node: "split", direction: "horizontal", children: [leaf("only")] };
  assert.deepEqual(canonicalize(n), leaf("only"));
});

t("canonicalize is idempotent", () => {
  const n: PanelLayoutNode = {
    node: "split",
    direction: "vertical",
    children: [leaf("a"), { node: "tabs", active: 0, children: [leaf("b"), leaf("c")] }],
  };
  const once = canonicalize(n);
  const twice = canonicalize(once);
  assert.deepEqual(once, twice);
});

t("empty plan → null model", () => {
  assert.equal(buildGridModel([]), null);
});

console.log(`dockviewAdapter: ${pass} passed, ${fail} failed`);
