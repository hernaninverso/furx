// services/window_reattach.rs — 018-fase-2-multiwindow-workspace · Phase B0 (T065, council-required)
//
// Semántica DETERMINISTA de close / reload / corrupción de layout (constitución VI:
// NUNCA matar procesos; layout corrupto → fallback sin pérdida).
//
// En la OLA 1 todavía no hay ventanas reales (eso es US2). Lo que se construye acá es
// el MODELO + la rama Main, transaccional, listo para que US2 lo invoque desde el
// window-close listener. Tres caminos, todos PUROS sobre `LayoutConfigV1` (testeables
// sin Tauri):
//
//   (A) CLOSE de una ventana detached → sus hojas se REATAN a Main (la ventana Main es
//       la dueña del ciclo; spec: cerrar reata, NO mata). `reattach_window_to_main`
//       mueve el subárbol de la ventana cerrada al final del Split raíz de Main y
//       elimina la WindowLayout cerrada. Determinista: orden estable, sin pérdida de
//       hojas. NUNCA toca procesos (sólo el árbol de layout).
//
//   (B) RELOAD de una webview con `?window_key=K` cuyo estado ya cambió (la ventana K
//       fue cerrada/reatada mientras la webview recargaba). `resolve_reload_target`
//       decide, mirando el SSOT:
//         - K existe en el layout → render normal de K (Reattach).
//         - K ya no existe (fue reatada a Main) → redirigir a Main (RedirectToMain).
//         - el layout está vacío/sin K y sin Main coherente → Empty (fallback seguro).
//
//   (C) CORRUPCIÓN / versión futura → `safe_or_fallback`: si el `LayoutConfigV1`
//       parseado es inválido (T062) o de una `version` futura desconocida, se cae a
//       `empty()` v1 SIN perder los procesos (los procesos viven en el backend US5;
//       el layout sólo describe la disposición). Nunca crashea.

use crate::services::layout_config::{
    LayoutConfigV1, PanelDescriptor, PanelLayoutNode, SplitDirection, WindowKind, WindowLayout,
    CURRENT_VERSION, MAIN_WINDOW_KEY,
};

/// A dónde debe renderizar una webview tras un reload, según el SSOT (caso B).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReloadTarget {
    /// `window_key` sigue existiendo → render normal de esa ventana.
    Reattach { window_key: String },
    /// `window_key` ya no existe (se cerró/reató) → la webview debe redirigir a Main.
    RedirectToMain,
    /// Ni `window_key` ni una Main coherente → render vacío (fallback seguro, sin crash).
    Empty,
}

/// Índice de la ventana Main en `windows`, si existe.
fn main_index(cfg: &LayoutConfigV1) -> Option<usize> {
    cfg.windows.iter().position(|w| w.kind == WindowKind::Main)
}

/// Hojas (Leaf) de un nodo, en orden, como subárboles a reatar. Aplana Split/Tabs
/// preservando cada Leaf individual (la unidad reatable de la ola 1 es el pane).
fn collect_leaves(node: &PanelLayoutNode, out: &mut Vec<PanelLayoutNode>) {
    match node {
        PanelLayoutNode::Leaf { .. } => out.push(node.clone()),
        PanelLayoutNode::Split { children, .. } | PanelLayoutNode::Tabs { children, .. } => {
            for c in children {
                collect_leaves(c, out);
            }
        }
    }
}

/// (A) CLOSE transaccional de una ventana detached: reata sus hojas a Main y la elimina
/// del layout. Devuelve un layout NUEVO (no muta el original). Determinista; preserva
/// todas las hojas (sin pérdida). NUNCA toca procesos. La revisión se incrementa (el
/// caller persiste vía `save`, que valida + chequea revisión).
///
/// Reglas:
///   - No-op (clon idéntico salvo revisión sin tocar) si `window_key` es la Main o no
///     existe: cerrar Main es política aparte (cerrar todo), y una key inexistente ya
///     fue manejada.
///   - Si Main no existe (layout corrupto), cae a `empty()` (defensa; no debería pasar
///     tras la validación, pero NUNCA perdemos las hojas sin un destino → mejor empty
///     que panic).
pub fn reattach_window_to_main(cfg: &LayoutConfigV1, window_key: &str) -> LayoutConfigV1 {
    let mut next = cfg.clone();
    // Cerrar Main no reata (política: Main es dueña; cerrar Main = cerrar todo, US2).
    if window_key == MAIN_WINDOW_KEY {
        return next;
    }
    let Some(closing_idx) = next.windows.iter().position(|w| w.window_key == window_key) else {
        // Ya no existe → no-op (caso ya resuelto).
        return next;
    };
    // Aislar las hojas de la ventana que cierra.
    let mut orphan_leaves = Vec::new();
    collect_leaves(&next.windows[closing_idx].layout, &mut orphan_leaves);
    // Quitar la ventana que cierra.
    next.windows.remove(closing_idx);

    let Some(mi) = main_index(&next) else {
        // No hay Main → fallback empty conservando workspace_id (no perdemos el archivo,
        // los procesos siguen vivos en el backend; el usuario reabre paneles).
        return LayoutConfigV1::empty(&next.workspace_id);
    };

    // Reatar las hojas al final del contenedor raíz de Main.
    match &mut next.windows[mi].layout {
        PanelLayoutNode::Split { children, .. } => children.extend(orphan_leaves),
        // Si el raíz de Main no es un Split (un solo Leaf/Tabs), lo envolvemos en un Split
        // horizontal con el contenido previo + las hojas reatadas (determinista).
        other => {
            let prev = other.clone();
            let mut children = vec![prev];
            children.extend(orphan_leaves);
            *other = PanelLayoutNode::Split {
                direction: SplitDirection::Horizontal,
                children,
            };
        }
    }
    next.revision = cfg.revision.saturating_add(1);
    next
}

/// Genera un `window_key` LIBRE para una ventana detached nueva, de la forma
/// `detached-N` con el menor N≥1 que no colisione con ningún `window_key` existente.
/// Determinista (busca el primer hueco), estable ante cierres (reusa N liberados).
pub fn next_detached_key(cfg: &LayoutConfigV1) -> String {
    let used: std::collections::HashSet<&str> =
        cfg.windows.iter().map(|w| w.window_key.as_str()).collect();
    let mut n = 1usize;
    loop {
        let candidate = format!("detached-{n}");
        if !used.contains(candidate.as_str()) {
            return candidate;
        }
        n += 1;
    }
}

/// Encuentra el `panel_type`+`params` de un panel_id en un nodo (para preservar el
/// descriptor al moverlo a la ventana detached). `None` si no está.
fn find_panel(node: &PanelLayoutNode, panel_id: &str) -> Option<PanelDescriptor> {
    match node {
        PanelLayoutNode::Leaf { panel } if panel.panel_id == panel_id => Some(panel.clone()),
        PanelLayoutNode::Leaf { .. } => None,
        PanelLayoutNode::Split { children, .. } | PanelLayoutNode::Tabs { children, .. } => {
            children.iter().find_map(|c| find_panel(c, panel_id))
        }
    }
}

/// Quita recursivamente la hoja `panel_id` del árbol. Devuelve `true` si quitó algo.
/// Tras quitar, NORMALIZA cada contenedor para no dejar formas que violarían la validación
/// T062 (Split anidado con <2 hijos, Tabs vacío):
///   - un contenedor que quedó VACÍO (0 hijos) se elimina de su padre;
///   - un contenedor ANIDADO que quedó con UN solo hijo se COLAPSA a ese hijo (un Split/Tabs
///     de 1 degenera). El raíz (Split contenedor de la ventana) tolera 0/1 hijos (validate_root),
///     así que NO se colapsa a nivel raíz — esa normalización la hace el caller que llama acá
///     sobre el nodo raíz de la ventana.
fn remove_leaf(node: &mut PanelLayoutNode, panel_id: &str) -> bool {
    match node {
        PanelLayoutNode::Leaf { .. } => false, // un Leaf raíz se maneja afuera.
        PanelLayoutNode::Tabs { active, children } => {
            // ¿es pestaña directa? (Leaf con ese id) → quitarla y clampear `active`.
            if let Some(idx) = children.iter().position(
                |c| matches!(c, PanelLayoutNode::Leaf { panel } if panel.panel_id == panel_id),
            ) {
                children.remove(idx);
                if *active >= children.len() && !children.is_empty() {
                    *active = children.len() - 1;
                }
                return true;
            }
            // recursar en hijos anidados con normalización (igual que Split).
            remove_from_children(children, panel_id)
        }
        PanelLayoutNode::Split { children, .. } => {
            // ¿es hijo directo? (Leaf con ese id)
            if let Some(idx) = children.iter().position(
                |c| matches!(c, PanelLayoutNode::Leaf { panel } if panel.panel_id == panel_id),
            ) {
                children.remove(idx);
                return true;
            }
            remove_from_children(children, panel_id)
        }
    }
}

/// Recursión + normalización compartida por Split y Tabs: baja a cada hijo contenedor y, tras
/// quitar, (a) elimina los que quedaron vacíos y (b) colapsa los que quedaron con un solo nieto.
fn remove_from_children(children: &mut Vec<PanelLayoutNode>, panel_id: &str) -> bool {
    let mut removed = false;
    let mut i = 0;
    while i < children.len() {
        if remove_leaf(&mut children[i], panel_id) {
            removed = true;
            if child_is_empty_container(&children[i]) {
                children.remove(i);
                continue;
            }
            if let Some(only) = single_child_container(&children[i]) {
                children[i] = only;
            }
        }
        i += 1;
    }
    removed
}

fn child_is_empty_container(node: &PanelLayoutNode) -> bool {
    matches!(
        node,
        PanelLayoutNode::Split { children, .. } | PanelLayoutNode::Tabs { children, .. } if children.is_empty()
    )
}

/// Si `node` es un Split/Tabs con EXACTAMENTE un hijo, devuelve un clon de ese hijo (para
/// colapsar el contenedor degenerado). `None` en cualquier otro caso.
fn single_child_container(node: &PanelLayoutNode) -> Option<PanelLayoutNode> {
    match node {
        PanelLayoutNode::Split { children, .. } | PanelLayoutNode::Tabs { children, .. }
            if children.len() == 1 =>
        {
            Some(children[0].clone())
        }
        _ => None,
    }
}

/// (D) DETACH transaccional de UN pane (Leaf) de Main a una NUEVA ventana detached.
/// Devuelve un layout NUEVO (no muta el original) + el `window_key` de la ventana creada
/// (None si no se pudo: el panel no está en Main, o es la única hoja y detacharla dejaría
/// Main vacío — permitido, queda Main con un Split vacío). NUNCA toca procesos: sólo mueve
/// el `PanelDescriptor` (panel_id incluido) de un árbol al otro. La revisión se incrementa.
///
/// Reglas:
///   - El panel DEBE existir en la ventana Main. Si no (ya detached, inexistente) → None.
///   - La nueva ventana es `Detached` con un `Split[h]` de 1 hoja (el pane movido).
///   - Main pierde esa hoja (con colapso de contenedores vacíos). El panel_id sigue siendo
///     único en toda la config (se MOVIÓ, no se duplicó) → la validación T062 pasa.
pub fn detach_panel_to_window(
    cfg: &LayoutConfigV1,
    panel_id: &str,
) -> Option<(LayoutConfigV1, String)> {
    let mut next = cfg.clone();
    let mi = main_index(&next)?;
    // El panel debe vivir en Main.
    let descriptor = find_panel(&next.windows[mi].layout, panel_id)?;
    // Quitarlo de Main.
    let removed = remove_leaf(&mut next.windows[mi].layout, panel_id);
    if !removed {
        return None;
    }
    // Crear la ventana detached con ese pane.
    let key = next_detached_key(&next);
    next.windows.push(WindowLayout {
        window_key: key.clone(),
        kind: WindowKind::Detached,
        display_hint: None,
        layout: PanelLayoutNode::Leaf { panel: descriptor },
    });
    next.revision = cfg.revision.saturating_add(1);
    Some((next, key))
}

/// (B) RELOAD: a dónde renderiza una webview que recarga con `?window_key=K`, según el
/// SSOT vigente. Determinista.
pub fn resolve_reload_target(cfg: &LayoutConfigV1, window_key: &str) -> ReloadTarget {
    if cfg.windows.iter().any(|w| w.window_key == window_key) {
        return ReloadTarget::Reattach {
            window_key: window_key.to_string(),
        };
    }
    // K ya no existe. Si hay una Main coherente, redirigir a ella; si no, vacío seguro.
    if cfg.windows.iter().any(|w| w.kind == WindowKind::Main) {
        ReloadTarget::RedirectToMain
    } else {
        ReloadTarget::Empty
    }
}

/// (C) CORRUPCIÓN / versión futura: dado el resultado de parsear un `LayoutConfigV1`
/// (Ok/Err) para `workspace_id`, devuelve un layout SEGURO. Si el parse falló (json
/// corrupto), o la `version` es desconocida (futura), o el árbol es inválido (T062),
/// cae a `empty()` v1. Nunca panic, nunca pierde procesos (viven en el backend).
pub fn safe_or_fallback(
    parsed: Result<LayoutConfigV1, String>,
    workspace_id: &str,
) -> LayoutConfigV1 {
    match parsed {
        Ok(cfg) if cfg.version == CURRENT_VERSION && cfg.validate().is_ok() => cfg,
        // version futura desconocida, o árbol inválido, o parse error → fallback v1 vacío.
        _ => LayoutConfigV1::empty(workspace_id),
    }
}

// ── US4 (T040) — ops de EDICIÓN del workspace (puras sobre LayoutConfigV1) ─────────────────────
// Mutación en RUST (SSOT unidireccional: UI→invoke→V1→LayoutChanged→re-hidrata; reorder T067).
// Todas devuelven un layout NUEVO (no mutan el original), incrementan `revision`, preservan la
// unicidad de panel_id (1 panel ↔ 1 vista) y NUNCA tocan procesos (VI). El caller persiste con
// `persist_layout_mutation` (que corre `validate()` T062 antes del write).

/// ¿Existe un Leaf con `panel_id` en ALGUNA ventana del layout? (chequeo de unicidad 1↔1).
pub fn panel_exists(cfg: &LayoutConfigV1, panel_id: &str) -> bool {
    cfg.windows
        .iter()
        .any(|w| find_panel(&w.layout, panel_id).is_some())
}

/// SPLIT: divide el pane `target_panel_id` reemplazando su Leaf por un
/// `Split{direction, [Leaf(target), Leaf(new_panel)]}` (el nuevo pane queda adyacente). `None` si
/// el target no existe, o si `new_panel.panel_id` ya está en el árbol (no duplicar panel_id). El
/// nuevo pane NO crea proceso: sólo inserta el descriptor; el lease/PTY lo cablea el front al montar
/// (igual que en detach).
pub fn split_pane(
    cfg: &LayoutConfigV1,
    target_panel_id: &str,
    direction: SplitDirection,
    new_panel: PanelDescriptor,
) -> Option<LayoutConfigV1> {
    if new_panel.panel_id == target_panel_id || panel_exists(cfg, &new_panel.panel_id) {
        return None; // no duplicar panel_id
    }
    let mut next = cfg.clone();
    let mut done = false;
    for w in next.windows.iter_mut() {
        if split_in_node(&mut w.layout, target_panel_id, direction, &new_panel) {
            done = true;
            break;
        }
    }
    if !done {
        return None; // target no encontrado en ninguna ventana
    }
    next.revision = cfg.revision.saturating_add(1);
    Some(next)
}

/// Reemplaza recursivamente el Leaf `target` por un Split{direction,[target,new]}. `true` si lo hizo.
fn split_in_node(
    node: &mut PanelLayoutNode,
    target: &str,
    direction: SplitDirection,
    new_panel: &PanelDescriptor,
) -> bool {
    match node {
        PanelLayoutNode::Leaf { panel } if panel.panel_id == target => {
            let original = panel.clone();
            *node = PanelLayoutNode::Split {
                direction,
                children: vec![
                    PanelLayoutNode::Leaf { panel: original },
                    PanelLayoutNode::Leaf {
                        panel: new_panel.clone(),
                    },
                ],
            };
            true
        }
        PanelLayoutNode::Leaf { .. } => false,
        PanelLayoutNode::Split { children, .. } | PanelLayoutNode::Tabs { children, .. } => {
            children
                .iter_mut()
                .any(|c| split_in_node(c, target, direction, new_panel))
        }
    }
}

/// CLOSE: quita el pane `panel_id` del layout (de cualquier ventana), normalizando contenedores
/// (sin Split/Tabs de <2 hijos anidados; `remove_leaf` ya colapsa los anidados, acá colapsamos la
/// raíz). Si era el Leaf RAÍZ de una ventana, la raíz queda como Split vacío (válido por
/// `validate_root`; la ventana existe pero sin panes). `None` si el pane no existe. NO mata el
/// proceso (VI): sólo lo saca de la vista; el front/lease decide el destino del PTY.
pub fn close_pane(cfg: &LayoutConfigV1, panel_id: &str) -> Option<LayoutConfigV1> {
    let mut next = cfg.clone();
    // Ventana que contiene el pane (en cualquiera de ellas).
    let widx = next
        .windows
        .iter()
        .position(|w| find_panel(&w.layout, panel_id).is_some())?;
    let is_main = matches!(next.windows[widx].kind, WindowKind::Main);
    {
        let w = &mut next.windows[widx];
        if matches!(&w.layout, PanelLayoutNode::Leaf { panel } if panel.panel_id == panel_id) {
            // raíz = Leaf bare → queda un Split vacío (lo resolvemos abajo según sea Main o no).
            w.layout = PanelLayoutNode::Split {
                direction: SplitDirection::Horizontal,
                children: vec![],
            };
        } else {
            remove_leaf(&mut w.layout, panel_id);
            // `remove_leaf` colapsa contenedores ANIDADOS de 1 hijo, pero NO la raíz → colapsar acá
            // si la raíz quedó como Split/Tabs de 1 hijo (contenedor degenerado a nivel raíz).
            if let Some(only) = single_child_container(&w.layout) {
                w.layout = only;
            }
        }
    }
    // ¿La ventana quedó SIN hojas? Main siempre existe → su forma vacía CANÓNICA es un `Split` vacío
    // (un `Tabs` vacío, p.ej. tras cerrar el único hijo de un `Tabs{[m1]}` raíz, es INVÁLIDO para
    // validate → normalizamos a Split vacío). Una DETACHED vacía es inválida y sin sentido → se
    // REMUEVE la ventana entera (el caller cerrará la WebviewWindow del SO al ver su window_key fuera
    // del layout). NUNCA mata el proceso del pane cerrado (VI): sólo lo saca de la vista.
    let mut leaves = vec![];
    collect_leaves(&next.windows[widx].layout, &mut leaves);
    if leaves.is_empty() {
        if is_main {
            next.windows[widx].layout = PanelLayoutNode::Split {
                direction: SplitDirection::Horizontal,
                children: vec![],
            };
        } else {
            next.windows.remove(widx);
        }
    }
    next.revision = cfg.revision.saturating_add(1);
    Some(next)
}

/// Extrae (remueve) el pane `panel_id` devolviendo su descriptor + el layout SIN él, con la misma
/// normalización que `close_pane` (colapsa contenedores; remueve detached vacía). `None` si no
/// existe. La `revision` la fija el caller (move/group) una sola vez. NUNCA toca el proceso.
fn extract_pane(cfg: &LayoutConfigV1, panel_id: &str) -> Option<(LayoutConfigV1, PanelDescriptor)> {
    let descriptor = cfg
        .windows
        .iter()
        .find_map(|w| find_panel(&w.layout, panel_id))?;
    let next = close_pane(cfg, panel_id)?;
    Some((next, descriptor))
}

/// MOVE: reubica el pane `panel_id` para que forme un `Split{direction}` con `target_panel_id`
/// (drag entre nodos). Extrae el pane de su lugar actual (normaliza/remueve detached vacía) y lo
/// inserta adyacente al target. `None` si son el mismo, si alguno no existe, o si el target
/// desapareció al extraer (no debería: son panes distintos). NUNCA mata procesos (VI).
pub fn move_pane(
    cfg: &LayoutConfigV1,
    panel_id: &str,
    target_panel_id: &str,
    direction: SplitDirection,
) -> Option<LayoutConfigV1> {
    if panel_id == target_panel_id || !panel_exists(cfg, target_panel_id) {
        return None;
    }
    let (mut next, descriptor) = extract_pane(cfg, panel_id)?;
    let mut done = false;
    for w in next.windows.iter_mut() {
        if split_in_node(&mut w.layout, target_panel_id, direction, &descriptor) {
            done = true;
            break;
        }
    }
    if !done {
        return None; // target no encontrado tras extraer → abort (caller no persiste)
    }
    next.revision = cfg.revision.saturating_add(1);
    Some(next)
}

/// GROUP-AS-TAB: agrupa el pane `panel_id` con `target_panel_id` en un `Tabs`. Si el target es un
/// Leaf suelto → nuevo `Tabs[target, moved]`; si el target ya es pestaña de un `Tabs` → se AGREGA a
/// ese Tabs (no anida). El pane movido queda como pestaña activa. `None` si son el mismo o falta
/// alguno. NUNCA mata procesos (VI).
pub fn group_as_tab(
    cfg: &LayoutConfigV1,
    panel_id: &str,
    target_panel_id: &str,
) -> Option<LayoutConfigV1> {
    if panel_id == target_panel_id || !panel_exists(cfg, target_panel_id) {
        return None;
    }
    let (mut next, descriptor) = extract_pane(cfg, panel_id)?;
    let mut done = false;
    for w in next.windows.iter_mut() {
        if group_in_node(&mut w.layout, target_panel_id, &descriptor) {
            done = true;
            break;
        }
    }
    if !done {
        return None;
    }
    next.revision = cfg.revision.saturating_add(1);
    Some(next)
}

/// Agrupa `moved` con el Leaf `target` en un Tabs. `true` si lo hizo. Si `target` ya es pestaña de
/// un Tabs, agrega `moved` a ESE Tabs (evita Tabs anidados); si es un Leaf suelto, crea un Tabs de 2.
fn group_in_node(node: &mut PanelLayoutNode, target: &str, moved: &PanelDescriptor) -> bool {
    match node {
        PanelLayoutNode::Leaf { panel } if panel.panel_id == target => {
            let original = panel.clone();
            *node = PanelLayoutNode::Tabs {
                active: 1,
                children: vec![
                    PanelLayoutNode::Leaf { panel: original },
                    PanelLayoutNode::Leaf {
                        panel: moved.clone(),
                    },
                ],
            };
            true
        }
        PanelLayoutNode::Leaf { .. } => false,
        PanelLayoutNode::Tabs { active, children } => {
            // ¿target es una pestaña directa de ESTE Tabs? → agregar moved acá (no anidar Tabs).
            if children
                .iter()
                .any(|c| matches!(c, PanelLayoutNode::Leaf { panel } if panel.panel_id == target))
            {
                children.push(PanelLayoutNode::Leaf {
                    panel: moved.clone(),
                });
                *active = children.len() - 1;
                return true;
            }
            children.iter_mut().any(|c| group_in_node(c, target, moved))
        }
        PanelLayoutNode::Split { children, .. } => {
            children.iter_mut().any(|c| group_in_node(c, target, moved))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::layout_config::PanelDescriptor;
    use serde_json::Value;

    fn leaf(id: &str) -> PanelLayoutNode {
        PanelLayoutNode::Leaf {
            panel: PanelDescriptor {
                panel_type: "terminal".to_string(),
                panel_id: id.to_string(),
                params: Value::Null,
            },
        }
    }

    /// Main (Split de 1 hoja "m1") + Detached (Split de 2 hojas d1,d2).
    fn cfg_main_plus_detached() -> LayoutConfigV1 {
        LayoutConfigV1 {
            version: CURRENT_VERSION,
            workspace_id: "ws".to_string(),
            revision: 5,
            windows: vec![
                WindowLayout {
                    window_key: MAIN_WINDOW_KEY.to_string(),
                    kind: WindowKind::Main,
                    display_hint: None,
                    layout: PanelLayoutNode::Split {
                        direction: SplitDirection::Horizontal,
                        children: vec![leaf("m1")],
                    },
                },
                WindowLayout {
                    window_key: "detached-1".to_string(),
                    kind: WindowKind::Detached,
                    display_hint: None,
                    layout: PanelLayoutNode::Split {
                        direction: SplitDirection::Vertical,
                        children: vec![leaf("d1"), leaf("d2")],
                    },
                },
            ],
        }
    }

    fn panel_ids(node: &PanelLayoutNode) -> Vec<String> {
        let mut v = Vec::new();
        collect_leaves(node, &mut v);
        v.into_iter()
            .filter_map(|n| match n {
                PanelLayoutNode::Leaf { panel } => Some(panel.panel_id),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn close_detached_reattaches_leaves_to_main_no_loss() {
        let cfg = cfg_main_plus_detached();
        let next = reattach_window_to_main(&cfg, "detached-1");
        // La ventana detached desaparece; queda sólo Main.
        assert_eq!(next.windows.len(), 1);
        assert_eq!(next.windows[0].kind, WindowKind::Main);
        // Las 3 hojas (m1 + d1 + d2) están en Main, sin pérdida, orden determinista.
        assert_eq!(panel_ids(&next.windows[0].layout), vec!["m1", "d1", "d2"]);
        // Revisión incrementada (write transaccional).
        assert_eq!(next.revision, cfg.revision + 1);
        // El resultado es válido (T062).
        assert!(next.validate().is_ok());
    }

    #[test]
    fn closing_main_is_noop_for_reattach() {
        let cfg = cfg_main_plus_detached();
        let next = reattach_window_to_main(&cfg, MAIN_WINDOW_KEY);
        assert_eq!(next.windows.len(), cfg.windows.len()); // política aparte (cerrar todo)
    }

    #[test]
    fn closing_unknown_window_is_noop() {
        let cfg = cfg_main_plus_detached();
        let next = reattach_window_to_main(&cfg, "no-existe");
        assert_eq!(next.windows.len(), cfg.windows.len());
    }

    #[test]
    fn reattach_wraps_non_split_main_root() {
        // Main con raíz Leaf (no Split): reatar debe envolver en un Split.
        let mut cfg = cfg_main_plus_detached();
        cfg.windows[0].layout = leaf("m1");
        let next = reattach_window_to_main(&cfg, "detached-1");
        assert!(matches!(
            next.windows[0].layout,
            PanelLayoutNode::Split { .. }
        ));
        assert_eq!(panel_ids(&next.windows[0].layout), vec!["m1", "d1", "d2"]);
        assert!(next.validate().is_ok());
    }

    #[test]
    fn reload_target_reattach_when_window_exists() {
        let cfg = cfg_main_plus_detached();
        assert_eq!(
            resolve_reload_target(&cfg, "detached-1"),
            ReloadTarget::Reattach {
                window_key: "detached-1".to_string()
            }
        );
    }

    #[test]
    fn reload_target_redirect_to_main_when_gone() {
        // La ventana detached ya fue reatada (cerrada); su webview recarga con su window_key viejo.
        let cfg = reattach_window_to_main(&cfg_main_plus_detached(), "detached-1");
        assert_eq!(
            resolve_reload_target(&cfg, "detached-1"),
            ReloadTarget::RedirectToMain
        );
    }

    #[test]
    fn reload_target_empty_when_no_main_and_no_window() {
        let cfg = LayoutConfigV1 {
            version: CURRENT_VERSION,
            workspace_id: "ws".to_string(),
            revision: 0,
            windows: vec![], // ni Main ni la window pedida
        };
        assert_eq!(
            resolve_reload_target(&cfg, "detached-9"),
            ReloadTarget::Empty
        );
    }

    #[test]
    fn next_detached_key_finds_first_free_slot() {
        let cfg = cfg_main_plus_detached(); // tiene "main" + "detached-1"
        assert_eq!(next_detached_key(&cfg), "detached-2");
        // Sin detached → detached-1.
        let mut only_main = cfg.clone();
        only_main.windows.truncate(1);
        assert_eq!(next_detached_key(&only_main), "detached-1");
    }

    #[test]
    fn detach_panel_moves_leaf_to_new_window_no_loss_no_dup() {
        // Main con un Split[h](m1, m2). Detachamos m2 → nueva ventana detached con m2;
        // Main queda con m1. El panel_id m2 sigue siendo ÚNICO (se movió, no se duplicó).
        let mut cfg = cfg_main_plus_detached();
        cfg.windows.truncate(1); // sólo Main
        cfg.windows[0].layout = PanelLayoutNode::Split {
            direction: SplitDirection::Horizontal,
            children: vec![leaf("m1"), leaf("m2")],
        };
        let (next, key) = detach_panel_to_window(&cfg, "m2").expect("m2 está en Main");
        assert_eq!(key, "detached-1");
        // Main perdió m2, conserva m1.
        let mi = main_index(&next).unwrap();
        assert_eq!(panel_ids(&next.windows[mi].layout), vec!["m1"]);
        // La nueva ventana detached tiene SÓLO m2.
        let det = next
            .windows
            .iter()
            .find(|w| w.window_key == "detached-1")
            .unwrap();
        assert_eq!(det.kind, WindowKind::Detached);
        assert_eq!(panel_ids(&det.layout), vec!["m2"]);
        // Revisión bump + árbol válido (panel_id único en toda la config).
        assert_eq!(next.revision, cfg.revision + 1);
        assert!(
            next.validate().is_ok(),
            "el árbol resultante es válido (T062)"
        );
    }

    #[test]
    fn detach_collapses_emptied_containers_and_keeps_valid() {
        // Main = Split[h]( m1, Split[v](m2, m3) ). Detachamos m2 → el Split[v] queda con
        // un solo hijo (m3): debe colapsar a Leaf(m3) para no violar "Split anidado ≥2".
        let mut cfg = cfg_main_plus_detached();
        cfg.windows.truncate(1);
        cfg.windows[0].layout = PanelLayoutNode::Split {
            direction: SplitDirection::Horizontal,
            children: vec![
                leaf("m1"),
                PanelLayoutNode::Split {
                    direction: SplitDirection::Vertical,
                    children: vec![leaf("m2"), leaf("m3")],
                },
            ],
        };
        let (next, _key) = detach_panel_to_window(&cfg, "m2").unwrap();
        let mi = main_index(&next).unwrap();
        // Main conserva m1 y m3 (m2 se fue). Estructura válida (sin Split de 1).
        let mut ids = panel_ids(&next.windows[mi].layout);
        ids.sort();
        assert_eq!(ids, vec!["m1", "m3"]);
        assert!(next.validate().is_ok());
    }

    #[test]
    fn detach_only_leaf_leaves_main_empty_valid() {
        // Main con una sola hoja m1; detacharla deja Main con un Split vacío (válido = ws vacío).
        let mut cfg = cfg_main_plus_detached();
        cfg.windows.truncate(1);
        cfg.windows[0].layout = PanelLayoutNode::Split {
            direction: SplitDirection::Horizontal,
            children: vec![leaf("m1")],
        };
        let (next, key) = detach_panel_to_window(&cfg, "m1").unwrap();
        let mi = main_index(&next).unwrap();
        assert!(
            panel_ids(&next.windows[mi].layout).is_empty(),
            "Main quedó vacía (válido)"
        );
        let det = next.windows.iter().find(|w| w.window_key == key).unwrap();
        assert_eq!(panel_ids(&det.layout), vec!["m1"]);
        assert!(next.validate().is_ok());
    }

    #[test]
    fn detach_returns_none_for_unknown_or_already_detached_panel() {
        let cfg = cfg_main_plus_detached(); // m1 en Main; d1/d2 en detached-1.
                                            // panel inexistente → None.
        assert!(detach_panel_to_window(&cfg, "no-existe").is_none());
        // un panel que YA vive en una ventana detached (no en Main) → None (no se re-detacha).
        assert!(detach_panel_to_window(&cfg, "d1").is_none());
    }

    #[test]
    fn detach_then_close_roundtrip_no_loss() {
        // Detach m2 a una ventana nueva, luego cerrarla reata m2 a Main: sin pérdida, válido.
        let mut cfg = cfg_main_plus_detached();
        cfg.windows.truncate(1);
        cfg.windows[0].layout = PanelLayoutNode::Split {
            direction: SplitDirection::Horizontal,
            children: vec![leaf("m1"), leaf("m2")],
        };
        let (after_detach, key) = detach_panel_to_window(&cfg, "m2").unwrap();
        let after_close = reattach_window_to_main(&after_detach, &key);
        // Main recupera m2; ya no hay detached.
        assert_eq!(after_close.windows.len(), 1);
        let mut ids = panel_ids(&after_close.windows[0].layout);
        ids.sort();
        assert_eq!(ids, vec!["m1", "m2"]);
        assert!(after_close.validate().is_ok());
    }

    #[test]
    fn corrupt_or_future_layout_falls_back_to_empty_v1() {
        // parse error → empty.
        let fb = safe_or_fallback(Err("json corrupto".to_string()), "ws");
        assert_eq!(fb.version, CURRENT_VERSION);
        assert_eq!(fb.workspace_id, "ws");
        assert!(fb.validate().is_ok());

        // version futura → empty (no intentamos renderizar un schema desconocido).
        let mut future = cfg_main_plus_detached();
        future.version = 999;
        let fb2 = safe_or_fallback(Ok(future), "ws");
        assert_eq!(fb2.version, CURRENT_VERSION);

        // árbol inválido (Tabs vacío) → empty.
        let mut invalid = cfg_main_plus_detached();
        invalid.windows[0].layout = PanelLayoutNode::Tabs {
            active: 0,
            children: vec![],
        };
        let fb3 = safe_or_fallback(Ok(invalid), "ws");
        assert_eq!(fb3.version, CURRENT_VERSION);
        assert!(fb3.validate().is_ok());

        // válido v1 → pasa sin tocar.
        let ok = cfg_main_plus_detached();
        let kept = safe_or_fallback(Ok(ok.clone()), "ws");
        assert_eq!(kept, ok);
    }

    // ── US4 (T040) — split / close ─────────────────────────────────────────────────────────────
    fn pd(id: &str) -> PanelDescriptor {
        PanelDescriptor {
            panel_type: "terminal".to_string(),
            panel_id: id.to_string(),
            params: Value::Null,
        }
    }

    #[test]
    fn split_pane_replaces_leaf_with_split_of_two() {
        // Main = Split{ Leaf(m1) }. Split de m1 (vertical) con un nuevo pane m2 → 2 hojas.
        let cfg = cfg_main_plus_detached();
        let out = split_pane(&cfg, "m1", SplitDirection::Vertical, pd("m2")).expect("split ok");
        assert_eq!(out.revision, cfg.revision + 1);
        assert!(panel_exists(&out, "m1") && panel_exists(&out, "m2"));
        assert!(out.validate().is_ok());
        let mut leaves = vec![];
        collect_leaves(&out.windows[0].layout, &mut leaves);
        assert_eq!(leaves.len(), 2, "m1 + m2 adyacentes en Main");
    }

    #[test]
    fn split_pane_rejects_duplicate_and_missing() {
        let cfg = cfg_main_plus_detached();
        // panel_id que ya existe (d1 vive en la detached) → None (no duplicar 1↔1).
        assert!(split_pane(&cfg, "m1", SplitDirection::Horizontal, pd("d1")).is_none());
        // new_panel == target → None.
        assert!(split_pane(&cfg, "m1", SplitDirection::Horizontal, pd("m1")).is_none());
        // target inexistente → None.
        assert!(split_pane(&cfg, "ghost", SplitDirection::Horizontal, pd("zz")).is_none());
    }

    #[test]
    fn close_pane_removes_and_collapses_container() {
        // Detached = Split{ Leaf(d1), Leaf(d2) }. Cerrar d1 → la raíz colapsa a Leaf(d2).
        let cfg = cfg_main_plus_detached();
        let out = close_pane(&cfg, "d1").expect("close ok");
        assert_eq!(out.revision, cfg.revision + 1);
        assert!(!panel_exists(&out, "d1") && panel_exists(&out, "d2"));
        assert!(out.validate().is_ok());
        assert!(
            matches!(&out.windows[1].layout, PanelLayoutNode::Leaf { panel } if panel.panel_id == "d2"),
            "Split de 1 hijo colapsa a ese hijo"
        );
    }

    #[test]
    fn close_pane_empties_root_split_when_last_child() {
        // Main = Split{ Leaf(m1) }. Cerrar m1 → Split raíz queda VACÍO (válido por validate_root).
        let cfg = cfg_main_plus_detached();
        let out = close_pane(&cfg, "m1").expect("close ok");
        assert!(!panel_exists(&out, "m1"));
        assert!(out.validate().is_ok());
        match &out.windows[0].layout {
            PanelLayoutNode::Split { children, .. } => assert!(children.is_empty()),
            other => panic!("esperaba Split vacío en Main, fue {other:?}"),
        }
    }

    #[test]
    fn close_pane_main_tabs_root_of_one_becomes_empty_split() {
        // audit codex: Main raíz = Tabs{[m1]} (válido). Cerrar m1 dejaría Tabs{[]} (0 hijos) que
        // validate RECHAZA → debe normalizarse a Split vacío (forma canónica del Main vacío).
        let mut cfg = cfg_main_plus_detached();
        cfg.windows[0].layout = PanelLayoutNode::Tabs {
            active: 0,
            children: vec![leaf("m1")],
        };
        let out = close_pane(&cfg, "m1").expect("close ok");
        assert!(!panel_exists(&out, "m1"));
        assert!(
            out.validate().is_ok(),
            "Main vacío debe ser válido (Split vacío, no Tabs vacío)"
        );
        assert!(
            matches!(&out.windows[0].layout, PanelLayoutNode::Split { children, .. } if children.is_empty())
        );
    }

    #[test]
    fn close_pane_last_pane_of_detached_removes_window() {
        // Cerrar el ÚLTIMO pane de una ventana DETACHED → la ventana entera se REMUEVE (una detached
        // vacía es inválida; Main en cambio sí puede quedar vacía). El proceso NO se mata (VI).
        let mut cfg = cfg_main_plus_detached();
        cfg.windows[1].layout = leaf("solo"); // detached con raíz Leaf (1 pane)
        assert_eq!(cfg.windows.len(), 2);
        let out = close_pane(&cfg, "solo").expect("close ok");
        assert!(!panel_exists(&out, "solo"));
        assert!(out.validate().is_ok());
        assert_eq!(out.windows.len(), 1, "la ventana detached vacía se removió");
        assert!(matches!(out.windows[0].kind, WindowKind::Main));
    }

    #[test]
    fn close_pane_last_pane_of_detached_with_container_removes_window() {
        // Igual que arriba pero la detached es Split{Leaf(d1),Leaf(d2)}: cerrar d1 colapsa a Leaf(d2)
        // (ventana sigue), cerrar d2 también → ventana removida.
        let cfg = cfg_main_plus_detached();
        let step1 = close_pane(&cfg, "d1").expect("close d1");
        assert_eq!(step1.windows.len(), 2, "aún queda d2 en la detached");
        let step2 = close_pane(&step1, "d2").expect("close d2");
        assert_eq!(step2.windows.len(), 1, "detached sin panes → removida");
        assert!(step2.validate().is_ok());
    }

    #[test]
    fn close_pane_missing_is_none() {
        let cfg = cfg_main_plus_detached();
        assert!(close_pane(&cfg, "ghost").is_none());
    }

    // ── US4 (T040) — move / group-as-tab ───────────────────────────────────────────────────────
    #[test]
    fn move_pane_relocates_next_to_target() {
        // Mover d1 (de la detached Split{d1,d2}) junto a m1 (Main), horizontal. d1 sale de la
        // detached (colapsa a Leaf(d2)); Main pasa a 2 hojas (m1 + d1). panel_id único preservado.
        let cfg = cfg_main_plus_detached();
        let out = move_pane(&cfg, "d1", "m1", SplitDirection::Horizontal).expect("move ok");
        assert_eq!(out.revision, cfg.revision + 1);
        assert!(panel_exists(&out, "d1") && panel_exists(&out, "m1") && panel_exists(&out, "d2"));
        assert!(out.validate().is_ok());
        let mut main_leaves = vec![];
        collect_leaves(&out.windows[0].layout, &mut main_leaves);
        assert_eq!(main_leaves.len(), 2, "m1 + d1 en Main");
        // La detached colapsó a Leaf(d2).
        assert!(
            matches!(&out.windows[1].layout, PanelLayoutNode::Leaf { panel } if panel.panel_id == "d2")
        );
    }

    #[test]
    fn move_pane_last_of_detached_removes_window() {
        // Mover el ÚNICO pane de una detached → la ventana se remueve y el pane sobrevive en Main.
        let mut cfg = cfg_main_plus_detached();
        cfg.windows[1].layout = leaf("solo");
        let out = move_pane(&cfg, "solo", "m1", SplitDirection::Vertical).expect("move ok");
        assert!(panel_exists(&out, "solo") && panel_exists(&out, "m1"));
        assert_eq!(out.windows.len(), 1, "detached vacía removida");
        assert!(out.validate().is_ok());
    }

    #[test]
    fn move_pane_rejects_self_and_missing() {
        let cfg = cfg_main_plus_detached();
        assert!(move_pane(&cfg, "m1", "m1", SplitDirection::Horizontal).is_none());
        assert!(move_pane(&cfg, "m1", "ghost", SplitDirection::Horizontal).is_none());
        assert!(move_pane(&cfg, "ghost", "m1", SplitDirection::Horizontal).is_none());
    }

    #[test]
    fn group_as_tab_creates_tabs_from_leaf() {
        // Agrupar d1 con m1 (Leaf suelto) → Main pasa a Tabs{m1, d1}, d1 activa.
        let cfg = cfg_main_plus_detached();
        let out = group_as_tab(&cfg, "d1", "m1").expect("group ok");
        assert!(panel_exists(&out, "m1") && panel_exists(&out, "d1") && panel_exists(&out, "d2"));
        assert!(out.validate().is_ok());
        // El nodo donde estaba m1 es ahora un Tabs de 2 con la pestaña movida activa.
        // Main era Split{Leaf(m1)} → el Leaf(m1) se volvió Tabs; tras colapsar el Split de 1,
        // la raíz puede ser el Tabs directamente o un Split conteniéndolo. Buscamos el Tabs.
        let node = &out.windows[0].layout;
        let mut found_tabs = false;
        fn has_tabs_of_two(n: &PanelLayoutNode, found: &mut bool) {
            match n {
                PanelLayoutNode::Tabs { children, active } => {
                    if children.len() == 2 && *active == 1 {
                        *found = true;
                    }
                    for c in children {
                        has_tabs_of_two(c, found);
                    }
                }
                PanelLayoutNode::Split { children, .. } => {
                    for c in children {
                        has_tabs_of_two(c, found);
                    }
                }
                PanelLayoutNode::Leaf { .. } => {}
            }
        }
        has_tabs_of_two(node, &mut found_tabs);
        assert!(
            found_tabs,
            "Main debe contener un Tabs de 2 con la pestaña movida activa"
        );
    }

    #[test]
    fn group_as_tab_appends_to_existing_tabs() {
        // Main = Tabs{m1, m2} (active 0). Agrupar d1 con m1 → se AGREGA a ese Tabs (no anida) → 3 tabs.
        let mut cfg = cfg_main_plus_detached();
        cfg.windows[0].layout = PanelLayoutNode::Tabs {
            active: 0,
            children: vec![leaf("m1"), leaf("m2")],
        };
        let out = group_as_tab(&cfg, "d1", "m1").expect("group ok");
        assert!(out.validate().is_ok());
        match &out.windows[0].layout {
            PanelLayoutNode::Tabs { children, active } => {
                assert_eq!(
                    children.len(),
                    3,
                    "d1 agregado al Tabs existente (sin anidar)"
                );
                assert_eq!(*active, 2, "la pestaña agregada queda activa");
            }
            other => panic!("esperaba Tabs en Main, fue {other:?}"),
        }
    }

    #[test]
    fn group_as_tab_rejects_self_and_missing() {
        let cfg = cfg_main_plus_detached();
        assert!(group_as_tab(&cfg, "m1", "m1").is_none());
        assert!(group_as_tab(&cfg, "m1", "ghost").is_none());
        assert!(group_as_tab(&cfg, "ghost", "m1").is_none());
    }
}
