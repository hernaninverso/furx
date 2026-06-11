// web/src/lib/dockviewAdapter.ts — 018-fase-2-multiwindow-workspace · Phase B1 (T067)
//
// ADAPTADOR bidireccional LayoutConfigV1 (SSOT) ↔ dockview (motor de render).
//
// REGLA DE ORO (council 5/5): `LayoutConfigV1` es el ÚNICO SSOT (Rust/DB). dockview es
// SÓLO el motor de render. NUNCA persistimos el JSON interno de dockview como schema —
// eso causaría split-brain. El flujo de mutación es UNIDIRECCIONAL:
//
//   UI edita (drag/split/close) → genera un LayoutCommand/patch → invoke Rust →
//   Rust valida (T062) + bump revision (T063) + persiste + emite UN LayoutChanged →
//   la(s) webview(s) re-hidratan: toDockview(node) → aplican el plan a su DockviewApi.
//
// Por eso NO usamos `dockview.toJSON()` como fuente: ese JSON trae tamaños/proporciones
// /orientación internos que NO viven en el SSOT. En su lugar:
//
//   - `toDockview(node)` → un PLAN determinista de operaciones `addPanel` (ordenado),
//     que el Workspace aplica imperativamente a una DockviewApi recién creada. Mapeo:
//       Split[horizontal] → panels colocados left→right (`direction: 'right'`)
//       Split[vertical]   → panels colocados top→bottom  (`direction: 'below'`)
//       Tabs              → panels en el MISMO grupo (stack)  (`position.referenceGroup`)
//       Leaf              → un panel `component` con params { panelId, panelType, ... }
//
//   - `fromDockview(model)` → reconstruye un `PanelLayoutNode` a partir de la ESTRUCTURA
//     del grid de dockview (branch/leaf + grupos), para capturar un drag del usuario de
//     vuelta al SSOT. Trabaja sobre un modelo NORMALIZADO (`DockviewGridModel`) que el
//     Workspace extrae del `DockviewApi` vivo — no del JSON interno crudo.
//
// El round-trip determinista se prueba sobre la FORMA CANÓNICA del árbol (un fold de
// splits unarios y la representación normalizada), no sobre bytes de dockview.

import type { PanelLayoutNode, PanelDescriptor } from "./layoutConfig.ts";

/** Dirección de colocación en dockview (sin `within`, que es el caso de Tabs/grupo). */
export type DockDirection = "left" | "right" | "above" | "below";

/** Una operación de construcción del layout de dockview. El Workspace la traduce a un
 *  `api.addPanel(...)`. Determinista y ordenada: aplicar el plan en orden reproduce el
 *  árbol. `position` ausente = primer panel (la raíz del grid). */
export interface DockviewAddOp {
  /** id del panel en dockview == panel_id del SSOT (1:1). */
  panelId: string;
  /** tipo de pane (claude/codex/terminal/...) → componente a montar. */
  panelType: string;
  /** params libres del PanelDescriptor (title/cwd/...). */
  params: unknown;
  /** posición relativa a un panel o grupo ya colocado. Ausente = raíz. */
  position?:
    | { type: "split"; referencePanelId: string; direction: DockDirection }
    | { type: "tab"; referenceGroupOf: string }; // mismo grupo que el panel ref (stack)
  /** Si este panel es la pestaña ACTIVA de su grupo (Tabs.active). Sólo el panel activo
   *  del grupo lo lleva `true`; preserva el índice de pestaña activa en el round-trip. */
  active?: boolean;
}

/** Plan completo (ordenado) que el Workspace aplica a una DockviewApi fresca. */
export type DockviewPlan = DockviewAddOp[];

// ── toDockview: PanelLayoutNode → plan de addPanel ────────────────────────────

/**
 * Convierte un `PanelLayoutNode` en un plan determinista de operaciones addPanel.
 * Recorrido en orden estable (DFS izquierda→derecha). El PRIMER leaf encontrado se
 * coloca como raíz (sin position); los siguientes se colocan relativos al "ancla"
 * de su contenedor:
 *   - dentro de un Split: cada hijo nuevo se ancla al PRIMER leaf del hijo anterior,
 *     con la dirección del split (right/below). Así un Split de N produce N-1 cortes.
 *   - dentro de un Tabs: todos los leaves comparten grupo con el primero (stack).
 */
export function toDockview(root: PanelLayoutNode): DockviewPlan {
  const plan: DockviewPlan = [];
  // first leaf placed → ancla raíz para el siguiente corte de nivel superior.
  let firstPlaced: string | null = null;
  // panel_ids que son la pestaña ACTIVA de su grupo (Tabs.active). Post-pass los marca.
  const activePanelIds = new Set<string>();

  // Devuelve el panelId del PRIMER leaf colocado por este subárbol (su "ancla").
  function walk(
    node: PanelLayoutNode,
    place: (op: DockviewAddOp) => void,
  ): string | null {
    switch (node.node) {
      case "leaf": {
        const id = node.panel.panelId;
        place({
          panelId: id,
          panelType: node.panel.panelType,
          params: node.panel.params ?? null,
          position: firstPlaced === null ? undefined : pendingPosition(id),
        });
        if (firstPlaced === null) firstPlaced = id;
        return id;
      }
      case "split": {
        const dir: DockDirection = node.direction === "vertical" ? "below" : "right";
        let firstAnchor: string | null = null;
        let prevAnchor: string | null = null;
        for (const child of node.children) {
          // El primer hijo del split se coloca con la posición pendiente (heredada);
          // los siguientes se cortan respecto del ancla del hijo PREVIO, con `dir`,
          // para que [a,b,c] quede a→b→c (no a→c→b).
          if (prevAnchor !== null) {
            nextSplit = { referencePanelId: prevAnchor, direction: dir };
          }
          const childAnchor = walk(child, place);
          if (firstAnchor === null) firstAnchor = childAnchor;
          prevAnchor = childAnchor;
        }
        return firstAnchor;
      }
      case "tabs": {
        let anchor: string | null = null;
        const activeIdx = Math.min(Math.max(node.active, 0), node.children.length - 1);
        node.children.forEach((child, i) => {
          // Todos los leaves del Tabs van al MISMO grupo (stack) que el primero.
          if (anchor !== null) {
            nextTabOf = anchor;
          }
          const childAnchor = walk(child, place);
          if (anchor === null) anchor = childAnchor;
          // Marca el panel ancla del hijo activo como la pestaña activa del grupo.
          if (i === activeIdx && childAnchor !== null) activePanelIds.add(childAnchor);
        });
        return anchor;
      }
    }
  }

  // Estado de "posición pendiente" para el próximo leaf (set por split/tabs antes de
  // recursar en un hijo). Se consume una sola vez (en pendingPosition).
  let nextSplit: { referencePanelId: string; direction: DockDirection } | null = null;
  let nextTabOf: string | null = null;
  function pendingPosition(_id: string): DockviewAddOp["position"] {
    if (nextTabOf !== null) {
      const ref = nextTabOf;
      nextTabOf = null;
      nextSplit = null;
      return { type: "tab", referenceGroupOf: ref };
    }
    if (nextSplit !== null) {
      const s = nextSplit;
      nextSplit = null;
      return { type: "split", referencePanelId: s.referencePanelId, direction: s.direction };
    }
    return undefined;
  }

  walk(root, (op) => plan.push(op));
  // Post-pass: marcar la pestaña activa de cada grupo.
  for (const op of plan) {
    if (activePanelIds.has(op.panelId)) op.active = true;
  }
  return plan;
}

// ── Modelo normalizado del grid de dockview (para fromDockview, testeable sin DOM) ──

/** Un grupo (stack/Tabs) de dockview: panels apilados + activo. */
export interface DockviewGroupModel {
  panels: { panelId: string; panelType: string; params: unknown }[];
  active: number;
}

/** Nodo del grid de dockview: branch (Split) o leaf (un grupo). Espejo NORMALIZADO de
 *  la estructura de dockview (branch/leaf), no del JSON crudo con tamaños. */
export type DockviewGridModel =
  | { kind: "branch"; orientation: "horizontal" | "vertical"; children: DockviewGridModel[] }
  | { kind: "group"; group: DockviewGroupModel };

// ── fromDockview: modelo normalizado del grid → PanelLayoutNode (SSOT) ────────

/**
 * Reconstruye el `PanelLayoutNode` a partir del modelo NORMALIZADO del grid de dockview
 * (lo que el Workspace extrae del DockviewApi vivo tras un drag del usuario). Inverso de
 * `toDockview` a nivel de FORMA:
 *   branch[horizontal] → Split{horizontal}, branch[vertical] → Split{vertical}
 *   group con 1 panel  → Leaf
 *   group con N panels → Tabs
 */
export function fromDockview(model: DockviewGridModel): PanelLayoutNode {
  switch (model.kind) {
    case "group": {
      const { panels, active } = model.group;
      if (panels.length === 1) {
        return { node: "leaf", panel: descriptorOf(panels[0]) };
      }
      return {
        node: "tabs",
        active: Math.min(Math.max(active, 0), panels.length - 1),
        children: panels.map((p) => ({ node: "leaf", panel: descriptorOf(p) })),
      };
    }
    case "branch": {
      return {
        node: "split",
        direction: model.orientation,
        children: model.children.map(fromDockview),
      };
    }
  }
}

function descriptorOf(p: { panelId: string; panelType: string; params: unknown }): PanelDescriptor {
  return { panelType: p.panelType, panelId: p.panelId, params: p.params ?? null };
}

// ── Normalización (forma canónica para round-trip determinista) ───────────────

/**
 * Forma CANÓNICA de un árbol para comparación round-trip: colapsa splits unarios (un
 * Split de 1 hijo == ese hijo) y splits anidados de la MISMA orientación (un
 * Split[h](a, Split[h](b,c)) se aplana a Split[h](a,b,c)) — dockview no distingue esas
 * dos formas (el grid las representa igual), así que el round-trip es determinista
 * SÓLO sobre la forma canónica. Idempotente.
 */
export function canonicalize(node: PanelLayoutNode): PanelLayoutNode {
  switch (node.node) {
    case "leaf":
      return node;
    case "tabs":
      return { node: "tabs", active: node.active, children: node.children.map(canonicalize) };
    case "split": {
      const flat: PanelLayoutNode[] = [];
      for (const c of node.children) {
        const cc = canonicalize(c);
        // Aplanar split anidado de igual orientación.
        if (cc.node === "split" && cc.direction === node.direction) {
          flat.push(...cc.children);
        } else {
          flat.push(cc);
        }
      }
      // Split unario → su único hijo.
      if (flat.length === 1) return flat[0];
      return { node: "split", direction: node.direction, children: flat };
    }
  }
}

/**
 * Construye el modelo NORMALIZADO del grid de dockview a partir de un plan (`toDockview`).
 * Es la simulación PURA de aplicar el plan a un DockviewApi (sin DOM): permite probar el
 * round-trip `node → plan → model → node` sin levantar dockview. El Workspace real aplica
 * el plan con `api.addPanel`; este builder reproduce la MISMA estructura branch/group.
 */
export function buildGridModel(plan: DockviewPlan): DockviewGridModel | null {
  if (plan.length === 0) return null;
  // Mapa panelId → grupo al que pertenece (para los tabs) + estructura branch.
  // Reconstrucción simple: cada op de split crea un branch; cada op de tab agrega al grupo.
  // Para un round-trip fiel a `toDockview`, reconstruimos en el MISMO orden.
  type MutGroup = { panels: { panelId: string; panelType: string; params: unknown }[]; active: number };
  const groups = new Map<string, MutGroup>(); // groupKey → group
  const groupOfPanel = new Map<string, string>(); // panelId → groupKey
  // Árbol de splits como lista de (parentAnchor, direction, childAnchor). Para la ola 1,
  // reconstruimos a partir de las posiciones, asumiendo el patrón que produce toDockview.
  interface Edge { ref: string; dir: DockDirection; child: string }
  const edges: Edge[] = [];
  let rootGroupKey: string | null = null;

  for (const op of plan) {
    if (!op.position) {
      // raíz: nuevo grupo.
      const gk = `g:${op.panelId}`;
      groups.set(gk, { panels: [{ panelId: op.panelId, panelType: op.panelType, params: op.params }], active: 0 });
      groupOfPanel.set(op.panelId, gk);
      rootGroupKey = gk;
    } else if (op.position.type === "tab") {
      // mismo grupo que el panel ref.
      const refGk = groupOfPanel.get(op.position.referenceGroupOf);
      if (refGk) {
        const g = groups.get(refGk)!;
        g.panels.push({ panelId: op.panelId, panelType: op.panelType, params: op.params });
        if (op.active) g.active = g.panels.length - 1;
        groupOfPanel.set(op.panelId, refGk);
      }
    } else {
      // split: nuevo grupo, edge desde el grupo del panel ref.
      const gk = `g:${op.panelId}`;
      groups.set(gk, { panels: [{ panelId: op.panelId, panelType: op.panelType, params: op.params }], active: 0 });
      groupOfPanel.set(op.panelId, gk);
      edges.push({ ref: op.position.referencePanelId, dir: op.position.direction, child: op.panelId });
    }
  }

  if (rootGroupKey === null) return null;

  // Reconstruir el árbol branch a partir de las edges. Cada edge agrega `child` como
  // hermano del subárbol que contiene a `ref`, en la orientación de `dir`.
  const orientationOf = (dir: DockDirection): "horizontal" | "vertical" =>
    dir === "left" || dir === "right" ? "horizontal" : "vertical";
  // groupKey → modelo de grupo (group node).
  const groupNode = (gk: string): DockviewGridModel => {
    const g = groups.get(gk)!;
    return { kind: "group", group: { panels: g.panels, active: g.active } };
  };
  const groupKeyOf = (pid: string): string => groupOfPanel.get(pid)!;

  // Construir incrementalmente: arrancamos con la raíz; aplicamos las edges en orden.
  let tree: DockviewGridModel = groupNode(rootGroupKey);
  for (const e of edges) {
    tree = insertSibling(tree, e.ref, groupNode(groupKeyOf(e.child)), orientationOf(e.dir));
  }
  return tree;
}

/** Inserta `newNode` como hermano del subárbol que contiene el grupo del panel `refPid`,
 *  bajo un branch de la orientación dada (aplanando si ya es esa orientación). */
function insertSibling(
  tree: DockviewGridModel,
  refPid: string,
  newNode: DockviewGridModel,
  orientation: "horizontal" | "vertical",
): DockviewGridModel {
  // ¿la raíz ES el grupo del ref?
  if (tree.kind === "group" && tree.group.panels.some((p) => p.panelId === refPid)) {
    return { kind: "branch", orientation, children: [tree, newNode] };
  }
  if (tree.kind === "branch") {
    // ¿algún hijo contiene el ref?
    const idx = tree.children.findIndex((c) => containsPanel(c, refPid));
    if (idx >= 0) {
      if (tree.orientation === orientation) {
        // misma orientación → agregar como hermano directo (aplanado).
        const children = [...tree.children];
        children.splice(idx + 1, 0, newNode);
        return { kind: "branch", orientation, children };
      }
      // orientación distinta → envolver el hijo en un branch nuevo.
      const children = [...tree.children];
      children[idx] = { kind: "branch", orientation, children: [children[idx], newNode] };
      return { kind: "branch", orientation: tree.orientation, children };
    }
  }
  // no encontrado → adjuntar a la raíz (defensivo, no debería pasar con planes válidos).
  return { kind: "branch", orientation, children: [tree, newNode] };
}

function containsPanel(t: DockviewGridModel, pid: string): boolean {
  if (t.kind === "group") return t.group.panels.some((p) => p.panelId === pid);
  return t.children.some((c) => containsPanel(c, pid));
}
