// web/src/lib/layoutTree.ts — 018-fase-2-multiwindow-workspace · US1 (T013)
//
// Mutadores PUROS del árbol de layout (`PanelLayoutNode`) + helpers find/replace.
// Toda mutación es PURA (no muta el input; devuelve un árbol nuevo) y NO persiste por
// sí sola — el caller genera un LayoutCommand y lo manda a Rust (flujo unidireccional
// del council: UI → LayoutCommand → invoke Rust → V1 valida+bump revision+persiste →
// LayoutChanged → re-hidrata). Acá viven SÓLO las transformaciones puras y testeables.
//
// SSOT: el árbol vive en `LayoutConfigV1` (Rust/DB). Estos helpers operan sobre un nodo
// (`PanelLayoutNode`) para que la UI calcule el árbol DESEADO y lo mande al backend.

import type {
  PanelLayoutNode,
  PanelDescriptor,
  SplitDirection,
} from "./layoutConfig.ts";

/** Recorre todas las hojas (Leaf) del árbol en orden DFS izquierda→derecha. */
export function leaves(node: PanelLayoutNode): PanelDescriptor[] {
  const out: PanelDescriptor[] = [];
  const walk = (n: PanelLayoutNode) => {
    if (n.node === "leaf") out.push(n.panel);
    else n.children.forEach(walk);
  };
  walk(node);
  return out;
}

/** Todos los panel_id del árbol (orden DFS). */
export function panelIds(node: PanelLayoutNode): string[] {
  return leaves(node).map((p) => p.panelId);
}

/** Encuentra la hoja con `panelId` (o null). */
export function findLeaf(node: PanelLayoutNode, panelId: string): PanelDescriptor | null {
  return leaves(node).find((p) => p.panelId === panelId) ?? null;
}

/**
 * Reemplaza (puro) la hoja `panelId` por `replacement` (un subárbol cualquiera).
 * Devuelve un árbol nuevo. Si `panelId` no existe, devuelve el árbol sin cambios
 * (referencialmente nuevo igual, para que el caller no dependa de identidad).
 */
export function replaceLeaf(
  node: PanelLayoutNode,
  panelId: string,
  replacement: PanelLayoutNode,
): PanelLayoutNode {
  switch (node.node) {
    case "leaf":
      return node.panel.panelId === panelId ? replacement : { ...node };
    case "split":
      return {
        node: "split",
        direction: node.direction,
        children: node.children.map((c) => replaceLeaf(c, panelId, replacement)),
      };
    case "tabs":
      return {
        node: "tabs",
        active: node.active,
        children: node.children.map((c) => replaceLeaf(c, panelId, replacement)),
      };
  }
}

/**
 * SPLIT (puro): divide la hoja `panelId` en un Split que la contiene + un pane nuevo
 * (`newPanel`), en la dirección dada. El pane existente queda primero; el nuevo, segundo.
 * Si `panelId` no es una hoja del árbol, no hay cambio.
 */
export function splitLeaf(
  node: PanelLayoutNode,
  panelId: string,
  direction: SplitDirection,
  newPanel: PanelDescriptor,
): PanelLayoutNode {
  const existing = findLeaf(node, panelId);
  if (!existing) return node;
  const split: PanelLayoutNode = {
    node: "split",
    direction,
    children: [
      { node: "leaf", panel: existing },
      { node: "leaf", panel: newPanel },
    ],
  };
  return replaceLeaf(node, panelId, split);
}

/**
 * SET RATIO de un Split (puro). En la ola 1 el ratio se modela como un campo libre en
 * `params` del nodo NO existe (el schema `Split` no lo tiene aún); dockview maneja el
 * sizing visual. Por eso `splitRatio` opera sobre una representación de ratios que el
 * caller mantiene aparte (Map nodePath→ratio). Acá exponemos sólo el helper de path
 * estable para que ese Map sea consistente entre renders. NO muta el árbol (el ratio no
 * vive en el SSOT del árbol; dockview lo persiste vía su propio sizing que NO es schema).
 *
 * Devuelve un PATH estable (índices) a un nodo, para indexar ratios fuera del árbol.
 */
export function nodePathToLeaf(node: PanelLayoutNode, panelId: string): number[] | null {
  const path: number[] = [];
  function walk(n: PanelLayoutNode): boolean {
    if (n.node === "leaf") return n.panel.panelId === panelId;
    for (let i = 0; i < n.children.length; i++) {
      path.push(i);
      if (walk(n.children[i])) return true;
      path.pop();
    }
    return false;
  }
  return walk(node) ? [...path] : null;
}

/**
 * CLOSE (puro): elimina la hoja `panelId` del árbol, reatando huérfanos. Si tras quitar
 * la hoja un Split/Tabs queda con un solo hijo, ese hijo lo reemplaza (colapso unario);
 * si queda con 0, se elimina el contenedor. NUNCA mata procesos (eso es decisión
 * explícita del usuario vía process_cancel; cerrar un pane del layout sólo lo saca de la
 * vista — el proceso sigue vivo en el backend, US5). Devuelve el árbol resultante o null
 * si el árbol queda VACÍO (el caller decide: empty workspace).
 */
export function closeLeaf(node: PanelLayoutNode, panelId: string): PanelLayoutNode | null {
  if (node.node === "leaf") {
    return node.panel.panelId === panelId ? null : node;
  }
  const kept = node.children
    .map((c) => closeLeaf(c, panelId))
    .filter((c): c is PanelLayoutNode => c !== null);
  if (kept.length === 0) return null;
  if (kept.length === 1 && node.node === "split") return kept[0]; // colapso unario de Split
  if (node.node === "tabs") {
    return {
      node: "tabs",
      active: Math.min(node.active, kept.length - 1),
      children: kept,
    };
  }
  return { node: "split", direction: node.direction, children: kept };
}

/**
 * GROUP-AS-TAB (puro): agrupa la hoja `panelId` con un pane nuevo (`newPanel`) en un
 * `Tabs` (stack). Reemplaza la hoja por un Tabs de [existing, new], con el nuevo activo.
 * Si `panelId` no existe, no hay cambio.
 */
export function groupAsTab(
  node: PanelLayoutNode,
  panelId: string,
  newPanel: PanelDescriptor,
): PanelLayoutNode {
  const existing = findLeaf(node, panelId);
  if (!existing) return node;
  const tabs: PanelLayoutNode = {
    node: "tabs",
    active: 1,
    children: [
      { node: "leaf", panel: existing },
      { node: "leaf", panel: newPanel },
    ],
  };
  return replaceLeaf(node, panelId, tabs);
}

// ── LayoutCommand (flujo de mutación UNIDIRECCIONAL) ──────────────────────────

/**
 * Un comando de mutación que la UI genera y manda al backend (NO muta dockview como
 * verdad). El backend lo aplica al SSOT, valida (T062), bump revision (T063), persiste y
 * emite UN LayoutChanged. En la ola 1 la UI calcula el árbol nuevo con los helpers de
 * arriba y manda un `SetLayout` (árbol completo de la ventana) — los comandos
 * granulares (split/close/move/tab) llegan con US4; el tipo ya los contempla.
 */
export type LayoutCommand =
  | { kind: "setWindowLayout"; windowKey: string; layout: PanelLayoutNode }
  | { kind: "split"; windowKey: string; panelId: string; direction: SplitDirection; newPanel: PanelDescriptor }
  | { kind: "close"; windowKey: string; panelId: string }
  | { kind: "groupAsTab"; windowKey: string; panelId: string; newPanel: PanelDescriptor };
