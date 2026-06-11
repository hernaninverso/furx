// services/layout_config.rs — 015-frontend-reform-kernel · US6
// Layout config versionada + multi-window-ready.
//
// CIMIENTO. No es la nav: vive EN PARALELO al `get_layout`/`save_layout` legacy
// (commands.rs, tabla `layouts`). La nav lo adopta en otra ola; hasta entonces
// este módulo sólo persiste/migra el schema versionado sin tocar lo viejo.
//
// Decisiones de diseño (spec US6):
//   - Schema VERSIONADO (`LayoutConfigV1 { version, .. }`) → migraciones de DATOS
//     dentro del json (v0→v1) además de las migraciones de DDL (027_*).
//   - `panel_type` (CLASE de panel, p.ej. "terminal"/"claude") ≠ `panel_id`
//     (INSTANCIA concreta). Nunca se confunden: dos panes del mismo tipo tienen
//     panel_type igual y panel_id distinto.
//   - Noción de `window_key` + `monitor` desde el día 1 aunque la UI use UNA
//     ventana: el schema serializa `Vec<WindowLayout>`.
//   - Display HINTS, no monitor-IDs absolutos: `display_hint.monitor_id` es un
//     hint opcional (puede no existir el monitor al rehidratar → se ignora y se
//     cae a la ventana principal). NUNCA se asume que el monitor existe.

use anyhow::{anyhow, Result};
use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::Arc;

type Db = Arc<parking_lot::Mutex<rusqlite::Connection>>;

/// Versión actual del schema de layout. Bump al introducir LayoutConfigV2 + su
/// migración de datos en `migrate_to_v1`-style.
pub const CURRENT_VERSION: u32 = 1;

/// Workspace por defecto cuando el caller no especifica uno (mono-workspace hoy).
pub const DEFAULT_WORKSPACE: &str = "default";

/// Ventana principal (la única que la UI usa en Fase 1). Las detached llegan en
/// Fase 2 pero el schema ya las soporta.
pub const MAIN_WINDOW_KEY: &str = "main";

// ── Schema tipado ────────────────────────────────────────────────────────────

/// Config de layout versionada de un workspace. Una fila en `layout_config`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LayoutConfigV1 {
    /// Versión del schema (== `CURRENT_VERSION` al guardar desde v1).
    pub version: u32,
    pub workspace_id: String,
    /// N ventanas. Hoy la UI usa 1 (`main`), pero el schema serializa varias.
    pub windows: Vec<WindowLayout>,
    /// 018 Fase 2 B0 (T063) — REVISIÓN monotónica (optimistic concurrency control). Toda
    /// mutación que persiste incrementa esto; `save` rechaza una escritura cuya `revision`
    /// no sea exactamente `stored + 1` (dos ventanas editando en paralelo no corrompen el
    /// árbol: el segundo writer ve `stale_layout` y re-lee). `serde(default)` = 0 para filas
    /// v1 viejas (schema 026 sin el campo) → migración transparente.
    #[serde(default)]
    pub revision: u64,
}

/// Clase de ventana. `Main` es la ventana raíz; `Detached` son ventanas
/// secundarias (Fase 2 multi-window). El schema las distingue desde el día 1.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WindowKind {
    Main,
    Detached,
}

/// Layout de UNA ventana: su clave estable, su clase, un hint de display y el
/// árbol de paneles.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WindowLayout {
    /// Clave ESTABLE de la ventana (p.ej. "main", "detached-1"). No es un handle
    /// de OS; sobrevive reloads.
    pub window_key: String,
    pub kind: WindowKind,
    /// Pista de posicionamiento. HINT, no autoridad: si el monitor no existe al
    /// rehidratar, se ignora. `None` = dejar que el WM decida.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_hint: Option<DisplayHint>,
    pub layout: PanelLayoutNode,
}

/// Pista de posición/tamaño de una ventana. Todos los campos opcionales: un hint
/// parcial es válido. `monitor_id` es un identificador BLANDO (best-effort match);
/// nunca un índice absoluto del que dependa la corrección.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DisplayHint {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub monitor_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub x: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub y: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<u32>,
}

/// Árbol de paneles de una ventana. Recursivo: hojas (`Leaf`) + nodos de
/// composición (`Split` horizontal/vertical, `Tabs` con pestaña activa).
/// `#[serde(tag = "node")]` → JSON discriminado por `"node": "leaf"|"split"|"tabs"`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "node", rename_all = "snake_case")]
pub enum PanelLayoutNode {
    /// Hoja: un panel concreto.
    Leaf { panel: PanelDescriptor },
    /// División en sub-árboles según `direction`.
    Split {
        direction: SplitDirection,
        children: Vec<PanelLayoutNode>,
    },
    /// Pestañas: `active` = índice de la pestaña activa (clamp en lectura).
    Tabs {
        active: usize,
        children: Vec<PanelLayoutNode>,
    },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SplitDirection {
    Horizontal,
    Vertical,
}

/// Descriptor de un panel. **`panel_type` = CLASE** (qué clase de panel:
/// "terminal", "claude", "codex", "zsh"...), **`panel_id` = INSTANCIA** (id único
/// de ESTE pane). `params` es metadata libre extensible por tipo de panel.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PanelDescriptor {
    pub panel_type: String,
    pub panel_id: String,
    #[serde(default)]
    pub params: Value,
}

impl LayoutConfigV1 {
    /// Config vacía válida (1 ventana Main sin paneles) para un workspace nuevo.
    pub fn empty(workspace_id: &str) -> Self {
        LayoutConfigV1 {
            version: CURRENT_VERSION,
            workspace_id: workspace_id.to_string(),
            windows: vec![WindowLayout {
                window_key: MAIN_WINDOW_KEY.to_string(),
                kind: WindowKind::Main,
                display_hint: None,
                // Split vacío = workspace sin paneles aún.
                layout: PanelLayoutNode::Split {
                    direction: SplitDirection::Horizontal,
                    children: vec![],
                },
            }],
            revision: 0,
        }
    }

    // ── T062 — Validador de árbol (council-required, BLOQUEANTE antes de CADA persist) ──

    /// Valida la config ANTES de un write. Reglas (council ADAPT, HIGH): cada una es un
    /// invariante de integridad del árbol, no estética. Devuelve `Err` con el motivo si
    /// alguna falla → `save` aborta SIN escribir (no se corrompe la DB).
    ///
    /// Reglas:
    ///   1. `panel_id` ÚNICO en TODA la config (cross-window) — un panel_id en dos hojas
    ///      rompe el mapeo Leaf→pane y el lease (T060) asume unicidad.
    ///   2. `Tabs` NO vacío — un grupo de pestañas sin hojas no renderiza nada.
    ///   3. `Split` con ≥2 hijos — un split de 1 (o 0) no es un split (degenera).
    ///   4. `window_key` no vacío y único entre ventanas.
    ///   5. Exactamente UNA ventana `Main` (las demás Detached).
    ///   6. Hojas alcanzables: una `Tabs.active` fuera de rango es inválida.
    ///   7. Un `Detached` no puede estar vacío (sin hojas) — sería una ventana fantasma.
    pub fn validate(&self) -> Result<()> {
        if self.windows.is_empty() {
            return Err(anyhow!("layout inválido: sin ventanas (al menos Main)"));
        }
        let mut window_keys = std::collections::HashSet::new();
        let mut main_count = 0usize;
        let mut all_panel_ids: Vec<String> = Vec::new();
        for w in &self.windows {
            if w.window_key.trim().is_empty() {
                return Err(anyhow!("layout inválido: window_key vacío"));
            }
            if !window_keys.insert(w.window_key.clone()) {
                return Err(anyhow!(
                    "layout inválido: window_key duplicado '{}'",
                    w.window_key
                ));
            }
            if w.kind == WindowKind::Main {
                main_count += 1;
            }
            let mut leaf_count = 0usize;
            validate_root(&w.layout, &mut all_panel_ids, &mut leaf_count)?;
            // Regla 7: una ventana Detached sin hojas es una ventana fantasma.
            if w.kind == WindowKind::Detached && leaf_count == 0 {
                return Err(anyhow!(
                    "layout inválido: ventana detached '{}' sin paneles",
                    w.window_key
                ));
            }
        }
        if main_count != 1 {
            return Err(anyhow!(
                "layout inválido: debe haber exactamente 1 ventana Main (hay {main_count})"
            ));
        }
        // Regla 1: panel_id único en toda la config.
        let mut seen = std::collections::HashSet::new();
        for pid in &all_panel_ids {
            if !seen.insert(pid.clone()) {
                return Err(anyhow!("layout inválido: panel_id duplicado '{pid}'"));
            }
        }
        Ok(())
    }
}

/// Valida el nodo RAÍZ de una ventana. El raíz es el CONTENEDOR de la ventana: un
/// `Split` raíz puede tener 0 hijos (workspace vacío — estado válido de `empty()`) o 1
/// (un solo pane, p.ej. tras `migrate_v0_to_v1` de un layout de 1 pane). La regla "Split
/// ≥2 hijos" sólo aplica a Splits ANIDADOS (un split interno de 1 sí degenera). Un `Tabs`
/// o `Leaf` raíz siguen las reglas normales (Tabs no vacío, etc.).
fn validate_root(
    node: &PanelLayoutNode,
    panel_ids: &mut Vec<String>,
    leaf_count: &mut usize,
) -> Result<()> {
    match node {
        PanelLayoutNode::Split { children, .. } => {
            // Raíz: tolera 0/1 hijos (vacío / single-pane). Los hijos sí se validan estrictos.
            for c in children {
                validate_node(c, panel_ids, leaf_count)?;
            }
            Ok(())
        }
        // Tabs/Leaf raíz: reglas normales.
        other => validate_node(other, panel_ids, leaf_count),
    }
}

/// Recorre un nodo ANIDADO validando reglas estructurales (T062) y recolectando
/// panel_ids + contando hojas. Recursivo.
fn validate_node(
    node: &PanelLayoutNode,
    panel_ids: &mut Vec<String>,
    leaf_count: &mut usize,
) -> Result<()> {
    match node {
        PanelLayoutNode::Leaf { panel } => {
            if panel.panel_id.trim().is_empty() {
                return Err(anyhow!("layout inválido: panel_id vacío en una hoja"));
            }
            panel_ids.push(panel.panel_id.clone());
            *leaf_count += 1;
            Ok(())
        }
        PanelLayoutNode::Split { children, .. } => {
            // Regla 3: un Split con <2 hijos degenera (no es una división).
            if children.len() < 2 {
                return Err(anyhow!(
                    "layout inválido: Split con {} hijo(s) (mínimo 2)",
                    children.len()
                ));
            }
            for c in children {
                validate_node(c, panel_ids, leaf_count)?;
            }
            Ok(())
        }
        PanelLayoutNode::Tabs { active, children } => {
            // Regla 2: Tabs vacío no renderiza nada.
            if children.is_empty() {
                return Err(anyhow!("layout inválido: Tabs vacío (sin pestañas)"));
            }
            // Regla 6: la pestaña activa debe existir.
            if *active >= children.len() {
                return Err(anyhow!(
                    "layout inválido: Tabs.active={} fuera de rango (len={})",
                    active,
                    children.len()
                ));
            }
            for c in children {
                validate_node(c, panel_ids, leaf_count)?;
            }
            Ok(())
        }
    }
}

// ── Migración de DATOS v0 → v1 ────────────────────────────────────────────────

/// Convierte el layout LEGACY (formato actual de panes: `layouts.panes` = array
/// de `{id, mode, title, cwd?, bundle_path?}` + grid) a un `LayoutConfigV1`
/// (single ventana `Main`). Mapeo:
///   - cada pane legacy → `Leaf { PanelDescriptor { panel_type: mode,
///     panel_id: id, params: { title, cwd?, bundle_path? } } }`
///   - todos los panes cuelgan de un único `Split` horizontal de la ventana main.
/// El `mode` legacy ("claude-A"/"codex"/"zsh"...) se usa como `panel_type`
/// (CLASE) y el `id` legacy como `panel_id` (INSTANCIA).
///
/// `legacy` es el JSON crudo del array `panes` (lo que hoy guarda `layouts.panes`).
/// Tolerante: panes malformados se saltean; un array vacío/ausente produce un
/// `LayoutConfigV1` válido vacío (no falla).
pub fn migrate_v0_to_v1(workspace_id: &str, legacy_panes: &Value) -> LayoutConfigV1 {
    let mut leaves: Vec<PanelLayoutNode> = Vec::new();
    if let Some(arr) = legacy_panes.as_array() {
        for (idx, p) in arr.iter().enumerate() {
            let Some(obj) = p.as_object() else { continue };
            // `mode` → panel_type (CLASE). Fallback "terminal" si falta.
            let panel_type = obj
                .get("mode")
                .and_then(|v| v.as_str())
                .unwrap_or("terminal")
                .to_string();
            // `id` → panel_id (INSTANCIA). Fallback determinístico por índice.
            let panel_id = obj
                .get("id")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .unwrap_or_else(|| format!("legacy-{idx}"));
            // params: arrastra title/cwd/bundle_path si están (sin perder contexto).
            let mut params = serde_json::Map::new();
            for k in ["title", "cwd", "bundle_path"] {
                if let Some(v) = obj.get(k) {
                    if !v.is_null() {
                        params.insert(k.to_string(), v.clone());
                    }
                }
            }
            leaves.push(PanelLayoutNode::Leaf {
                panel: PanelDescriptor {
                    panel_type,
                    panel_id,
                    params: Value::Object(params),
                },
            });
        }
    }
    LayoutConfigV1 {
        version: CURRENT_VERSION,
        workspace_id: workspace_id.to_string(),
        windows: vec![WindowLayout {
            window_key: MAIN_WINDOW_KEY.to_string(),
            kind: WindowKind::Main,
            display_hint: None,
            layout: PanelLayoutNode::Split {
                direction: SplitDirection::Horizontal,
                children: leaves,
            },
        }],
        revision: 0,
    }
}

// ── Persistencia ──────────────────────────────────────────────────────────────

/// Carga la config de layout de un workspace. Si NO existe una fila v1 todavía,
/// intenta MIGRAR el layout legacy (`layouts.panes`) a v1 (sin persistirlo:
/// persistir es decisión del caller / próxima ola). Si tampoco hay legacy,
/// devuelve un `LayoutConfigV1::empty`. Nunca falla por "no hay layout".
pub fn get(db: &Db, workspace_id: &str) -> Result<LayoutConfigV1> {
    let conn = db.lock();
    // 1) ¿ya hay una config v1 persistida?
    let stored: Option<String> = conn
        .query_row(
            "SELECT json FROM layout_config WHERE workspace_id = ?1",
            rusqlite::params![workspace_id],
            |r| r.get::<_, String>(0),
        )
        .ok();
    if let Some(json) = stored {
        let cfg: LayoutConfigV1 = serde_json::from_str(&json)
            .map_err(|e| anyhow!("layout_config json inválido para {workspace_id}: {e}"))?;
        // MED-4 (audit): VALIDAR antes de devolver al front. Un árbol corrupto (panel_id
        // duplicado, Tabs vacío, etc.) o una `version` FUTURA (escrita por un build más nuevo
        // que no entendemos) NO debe llegar a dockview — rompería el render / el mapeo de leases.
        // Fail-closed: caemos a `empty()` (Main vacío, válido) y logueamos. NO sobrescribimos la
        // fila persistida (el `save` próximo lo hará con la revisión correcta); sólo evitamos
        // servir basura. El proceso PTY NO se toca (esta función no tiene acceso a él).
        if cfg.version > CURRENT_VERSION {
            tracing::warn!(
                workspace_id,
                stored_version = cfg.version,
                current = CURRENT_VERSION,
                "layout_config con version futura — cae a layout vacío (fallback)"
            );
            return Ok(LayoutConfigV1::empty(workspace_id));
        }
        if let Err(e) = cfg.validate() {
            tracing::warn!(workspace_id, error = %e, "layout_config inválido al leer — cae a layout vacío (fallback)");
            return Ok(LayoutConfigV1::empty(workspace_id));
        }
        return Ok(cfg);
    }
    // 2) sin v1 → intentar migrar desde el layout legacy.
    //    El legacy se identifica por id="default" en la tabla `layouts` (mono-ws).
    let legacy_panes: Option<String> = conn
        .query_row(
            "SELECT panes FROM layouts WHERE id = ?1",
            rusqlite::params![legacy_layout_id(workspace_id)],
            |r| r.get::<_, String>(0),
        )
        .ok();
    drop(conn);
    if let Some(panes_json) = legacy_panes {
        let panes: Value = serde_json::from_str(&panes_json).unwrap_or(Value::Null);
        return Ok(migrate_v0_to_v1(workspace_id, &panes));
    }
    // 3) nada → vacío válido.
    Ok(LayoutConfigV1::empty(workspace_id))
}

/// Persiste (upsert) la config de layout de un workspace.
///
/// 018 Fase 2 B0:
///   - T062: VALIDA el árbol ANTES de escribir. Una config inválida (panel_id
///     duplicado, Tabs vacío, Split anidado <2, etc.) devuelve `Err` SIN tocar la DB.
///   - T063: REVISIÓN monotónica (optimistic concurrency). La `cfg.revision` debe ser
///     exactamente `stored_revision + 1`. Si no (otra ventana escribió primero, o
///     `revision` stale), devuelve `Err("stale_layout: ...")` SIN escribir — el caller
///     re-lee y reaplica su mutación sobre la revisión nueva. Dos escritores
///     concurrentes nunca corrompen el árbol: a lo sumo uno gana cada vuelta.
///
/// El guard de revisión se hace en una transacción IMMEDIATE para serializar el
/// read-check-write contra otros writers de la MISMA conexión/proceso.
pub fn save(db: &Db, cfg: &LayoutConfigV1) -> Result<()> {
    // T062 — validación previa al write. Fail-closed.
    cfg.validate()?;
    let json = serde_json::to_string(cfg).map_err(|e| anyhow!("serialize layout_config: {e}"))?;
    let mut conn = db.lock();
    let tx = conn
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(|e| anyhow!("begin tx: {e}"))?;
    // Revisión persistida (0 si la fila no existe todavía → primer write debe ser revision=1).
    // MED-3 (audit): NO `as u64` (un i64 negativo se volvería un u64 GIGANTE y el guard
    // `cfg.revision != stored+1` pasaría con basura). Una revisión negativa en la DB sólo puede
    // venir de corrupción/manipulación → la tratamos COMO TAL (fail-closed): error explícito sin
    // escribir. `u64::try_from` rechaza el negativo.
    let stored_row: Option<i64> = tx
        .query_row(
            "SELECT revision FROM layout_config WHERE workspace_id = ?1",
            rusqlite::params![cfg.workspace_id],
            |r| r.get::<_, i64>(0),
        )
        .optional()
        .map_err(|e| anyhow!("read revision: {e}"))?;
    let stored_rev: u64 = match stored_row {
        None => 0, // fila inexistente → primer write debe ser revision=1.
        Some(v) => u64::try_from(v).map_err(|_| {
            anyhow!(
                "layout_config corrupto: revision negativa ({v}) para {} — no se escribe",
                cfg.workspace_id
            )
        })?,
    };
    // T063 — el write debe avanzar la revisión EXACTAMENTE en 1. Cualquier otra cosa es stale.
    // MED-3: `checked_add` evita un overflow silencioso de `stored + 1` en el límite de u64.
    let expected = stored_rev.checked_add(1).ok_or_else(|| {
        anyhow!(
            "layout_config: revision desbordada para {}",
            cfg.workspace_id
        )
    })?;
    if cfg.revision != expected {
        return Err(anyhow!(
            "stale_layout: revision esperada {} (stored {}), recibida {} — re-leer y reaplicar",
            expected,
            stored_rev,
            cfg.revision
        ));
    }
    // MED-3: al ESCRIBIR usamos `i64::try_from` (no `as i64`) — una revision que no entre en i64
    // (>i64::MAX) es corrupción de estado, no la persistimos como un valor truncado.
    let revision_i64 = i64::try_from(cfg.revision).map_err(|_| {
        anyhow!(
            "layout_config: revision {} no representable como i64",
            cfg.revision
        )
    })?;
    tx.execute(
        "INSERT INTO layout_config (workspace_id, version, json, revision) VALUES (?1, ?2, ?3, ?4) \
         ON CONFLICT(workspace_id) DO UPDATE SET version = excluded.version, \
            json = excluded.json, revision = excluded.revision, updated_at = datetime('now')",
        rusqlite::params![cfg.workspace_id, cfg.version, json, revision_i64],
    )
    .map_err(|e| anyhow!("save layout_config: {e}"))?;
    tx.commit().map_err(|e| anyhow!("commit: {e}"))?;
    Ok(())
}

/// Id de la fila legacy en `layouts` para un workspace. Hoy mono-workspace: el
/// default usa "default". (Cuando haya workspaces reales, este mapeo se afina.)
fn legacy_layout_id(workspace_id: &str) -> &str {
    if workspace_id == DEFAULT_WORKSPACE {
        "default"
    } else {
        workspace_id
    }
}

// ── Tauri commands (viven en paralelo a get_layout/save_layout legacy) ────────

/// Devuelve la config de layout versionada del workspace (default si no se pasa).
#[tauri::command]
pub fn layout_config_get(
    state: tauri::State<'_, crate::AppState>,
    workspace_id: Option<String>,
) -> Result<LayoutConfigV1, String> {
    let ws = workspace_id.unwrap_or_else(|| DEFAULT_WORKSPACE.to_string());
    get(&state.db, &ws).map_err(|e| e.to_string())
}

/// Persiste la config de layout versionada.
#[tauri::command]
pub fn layout_config_save(
    state: tauri::State<'_, crate::AppState>,
    config: LayoutConfigV1,
) -> Result<(), String> {
    save(&state.db, &config).map_err(|e| e.to_string())
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn test_db() -> Db {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        // Tabla nueva (026) + revisión 031 (T063) + la legacy `layouts` (002) para la migración.
        conn.execute_batch(include_str!("../../migrations/026_layout_config.sql"))
            .unwrap();
        conn.execute_batch(include_str!("../../migrations/031_layout_revision.sql"))
            .unwrap();
        conn.execute_batch(include_str!("../../migrations/002_settings.sql"))
            .unwrap();
        Arc::new(parking_lot::Mutex::new(conn))
    }

    fn sample_config() -> LayoutConfigV1 {
        LayoutConfigV1 {
            version: CURRENT_VERSION,
            workspace_id: "ws-1".to_string(),
            revision: 1,
            windows: vec![
                WindowLayout {
                    window_key: MAIN_WINDOW_KEY.to_string(),
                    kind: WindowKind::Main,
                    display_hint: Some(DisplayHint {
                        monitor_id: Some("hdmi-1".to_string()),
                        x: Some(0),
                        y: Some(0),
                        width: Some(1920),
                        height: Some(1080),
                    }),
                    layout: PanelLayoutNode::Split {
                        direction: SplitDirection::Vertical,
                        children: vec![
                            PanelLayoutNode::Leaf {
                                panel: PanelDescriptor {
                                    panel_type: "claude".to_string(),
                                    panel_id: "p1".to_string(),
                                    params: serde_json::json!({"title": "Pane 1"}),
                                },
                            },
                            PanelLayoutNode::Tabs {
                                active: 1,
                                children: vec![
                                    PanelLayoutNode::Leaf {
                                        panel: PanelDescriptor {
                                            panel_type: "codex".to_string(),
                                            panel_id: "p2".to_string(),
                                            params: Value::Null,
                                        },
                                    },
                                    PanelLayoutNode::Leaf {
                                        panel: PanelDescriptor {
                                            panel_type: "zsh".to_string(),
                                            panel_id: "p3".to_string(),
                                            params: Value::Null,
                                        },
                                    },
                                ],
                            },
                        ],
                    },
                },
                // Segunda ventana DETACHED: el schema la soporta aunque la UI use 1.
                WindowLayout {
                    window_key: "detached-1".to_string(),
                    kind: WindowKind::Detached,
                    display_hint: None,
                    layout: PanelLayoutNode::Leaf {
                        panel: PanelDescriptor {
                            panel_type: "terminal".to_string(),
                            panel_id: "p4".to_string(),
                            params: Value::Null,
                        },
                    },
                },
            ],
        }
    }

    #[test]
    fn schema_roundtrips_multiple_windows() {
        // Serializa/deserializa una config con 2 ventanas (Main + Detached),
        // splits, tabs y display hints — aunque la UI use una sola ventana.
        let cfg = sample_config();
        let json = serde_json::to_string(&cfg).unwrap();
        let back: LayoutConfigV1 = serde_json::from_str(&json).unwrap();
        assert_eq!(cfg, back);
        assert_eq!(back.windows.len(), 2);
        assert_eq!(back.windows[0].kind, WindowKind::Main);
        assert_eq!(back.windows[1].kind, WindowKind::Detached);
    }

    #[test]
    fn panel_type_is_class_not_instance() {
        // panel_type (CLASE) ≠ panel_id (INSTANCIA): dos panes "terminal"
        // comparten panel_type pero NO panel_id.
        let a = PanelDescriptor {
            panel_type: "terminal".to_string(),
            panel_id: "inst-a".to_string(),
            params: Value::Null,
        };
        let b = PanelDescriptor {
            panel_type: "terminal".to_string(),
            panel_id: "inst-b".to_string(),
            params: Value::Null,
        };
        assert_eq!(a.panel_type, b.panel_type);
        assert_ne!(a.panel_id, b.panel_id);
    }

    #[test]
    fn persists_with_version_and_window_key() {
        let db = test_db();
        let cfg = sample_config();
        save(&db, &cfg).unwrap();
        let loaded = get(&db, "ws-1").unwrap();
        assert_eq!(loaded.version, CURRENT_VERSION);
        assert_eq!(loaded.windows[0].window_key, MAIN_WINDOW_KEY);
        assert_eq!(loaded, cfg);
        // La fila guarda `version` como columna propia (no sólo dentro del json).
        let conn = db.lock();
        let v: u32 = conn
            .query_row(
                "SELECT version FROM layout_config WHERE workspace_id = ?1",
                rusqlite::params!["ws-1"],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(v, CURRENT_VERSION);
    }

    #[test]
    fn save_is_idempotent_upsert() {
        let db = test_db();
        let mut cfg = sample_config();
        save(&db, &cfg).unwrap();
        cfg.windows.truncate(1); // mutar y re-guardar
        cfg.revision = 2; // T063: el re-save debe avanzar la revisión.
        save(&db, &cfg).unwrap();
        let loaded = get(&db, "ws-1").unwrap();
        assert_eq!(loaded.windows.len(), 1);
        // sigue habiendo UNA sola fila para el workspace.
        let conn = db.lock();
        let n: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM layout_config WHERE workspace_id = ?1",
                rusqlite::params!["ws-1"],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 1);
    }

    #[test]
    fn migration_v0_to_v1_produces_single_main_window() {
        // El formato legacy actual de `layouts.panes`.
        let legacy = serde_json::json!([
            {"id": "p1", "mode": "claude-A", "title": "Pane 1"},
            {"id": "p2", "mode": "codex", "title": "Pane 2", "cwd": "/tmp/x"},
            {"id": "p3", "mode": "zsh", "title": "Pane 3"}
        ]);
        let cfg = migrate_v0_to_v1("default", &legacy);
        assert_eq!(cfg.version, CURRENT_VERSION);
        assert_eq!(cfg.workspace_id, "default");
        // single ventana Main.
        assert_eq!(cfg.windows.len(), 1);
        assert_eq!(cfg.windows[0].kind, WindowKind::Main);
        assert_eq!(cfg.windows[0].window_key, MAIN_WINDOW_KEY);
        // 3 hojas bajo un split horizontal; mode→panel_type, id→panel_id.
        let PanelLayoutNode::Split {
            direction,
            children,
        } = &cfg.windows[0].layout
        else {
            panic!("expected split");
        };
        assert_eq!(*direction, SplitDirection::Horizontal);
        assert_eq!(children.len(), 3);
        let PanelLayoutNode::Leaf { panel } = &children[0] else {
            panic!("expected leaf");
        };
        assert_eq!(panel.panel_type, "claude-A"); // CLASE = mode legacy
        assert_eq!(panel.panel_id, "p1"); // INSTANCIA = id legacy
        assert_eq!(panel.params["title"], serde_json::json!("Pane 1"));
        // el cwd del pane 2 se arrastra a params.
        let PanelLayoutNode::Leaf { panel: p2 } = &children[1] else {
            panic!("expected leaf");
        };
        assert_eq!(p2.params["cwd"], serde_json::json!("/tmp/x"));
        // y el resultado es serializable/deserializable.
        let json = serde_json::to_string(&cfg).unwrap();
        let back: LayoutConfigV1 = serde_json::from_str(&json).unwrap();
        assert_eq!(cfg, back);
    }

    #[test]
    fn migration_handles_empty_and_malformed() {
        // array vacío → config válida vacía (un split sin children).
        let cfg = migrate_v0_to_v1("default", &serde_json::json!([]));
        let PanelLayoutNode::Split { children, .. } = &cfg.windows[0].layout else {
            panic!("expected split");
        };
        assert!(children.is_empty());
        // null / no-array → también válido (no panic).
        let cfg2 = migrate_v0_to_v1("default", &Value::Null);
        assert_eq!(cfg2.windows.len(), 1);
        // pane sin id → panel_id determinístico por índice; sin mode → "terminal".
        let cfg3 = migrate_v0_to_v1("default", &serde_json::json!([{"title": "x"}]));
        let PanelLayoutNode::Split { children, .. } = &cfg3.windows[0].layout else {
            panic!("expected split");
        };
        let PanelLayoutNode::Leaf { panel } = &children[0] else {
            panic!("expected leaf");
        };
        assert_eq!(panel.panel_type, "terminal");
        assert_eq!(panel.panel_id, "legacy-0");
    }

    #[test]
    fn get_falls_back_to_legacy_migration_then_empty() {
        let db = test_db();
        // sembrar un layout legacy "default" en la tabla `layouts`.
        {
            let conn = db.lock();
            conn.execute(
                "INSERT INTO layouts (id, name, panes) VALUES ('default', 'L', ?1)",
                rusqlite::params![r#"[{"id":"p1","mode":"zsh","title":"T"}]"#],
            )
            .unwrap();
        }
        // sin fila v1 → get migra desde legacy (single Main window).
        let migrated = get(&db, "default").unwrap();
        assert_eq!(migrated.version, CURRENT_VERSION);
        assert_eq!(migrated.windows.len(), 1);
        let PanelLayoutNode::Split { children, .. } = &migrated.windows[0].layout else {
            panic!("expected split");
        };
        assert_eq!(children.len(), 1);

        // un workspace sin legacy ni v1 → empty válido.
        let empty = get(&db, "ws-sin-nada").unwrap();
        assert_eq!(empty.version, CURRENT_VERSION);
        assert_eq!(empty.windows.len(), 1);
        assert_eq!(empty.windows[0].kind, WindowKind::Main);
    }

    #[test]
    fn save_treats_negative_stored_revision_as_corruption() {
        // MED-3 (audit): una fila con revision NEGATIVA (corrupción/manipulación) NO debe pasar
        // el guard como un u64 gigante (lo que `as u64` haría). `u64::try_from` la rechaza →
        // `save` devuelve Err sin escribir. (No se toca ningún proceso.)
        let db = test_db();
        {
            let conn = db.lock();
            conn.execute(
                "INSERT INTO layout_config (workspace_id, version, json, revision) VALUES ('ws-neg', 1, '{}', -5)",
                [],
            )
            .unwrap();
        }
        let mut cfg = LayoutConfigV1::empty("ws-neg");
        // Con `as u64`, stored=-5 → u64::MAX-4, esperado = u64::MAX-3; el atacante podría calzar
        // esa revisión y escribir. Probamos que CUALQUIER revisión es rechazada por corrupción.
        cfg.revision = u64::MAX - 3;
        let err = save(&db, &cfg).unwrap_err().to_string();
        assert!(
            err.contains("corrupto") && err.contains("negativa"),
            "esperaba error de corrupción, got: {err}"
        );
        // revision=1 tampoco pasa (la fila sigue corrupta, no se puede derivar un esperado válido).
        cfg.revision = 1;
        assert!(
            save(&db, &cfg).is_err(),
            "una fila con revision negativa nunca debe permitir un write"
        );
    }

    #[test]
    fn get_returns_fallback_for_corrupt_tree_and_future_version() {
        // MED-4 (audit): un árbol persistido INVÁLIDO (panel_id duplicado) NO debe llegar al
        // front — `get` valida y cae a `empty()` (Main vacío, v1). Igual para una `version`
        // FUTURA (build más nuevo). No corrompe ni reescribe la fila; sólo evita servir basura.
        let db = test_db();
        // 1) Árbol con panel_id DUPLICADO en dos hojas (rompe el mapeo Leaf→pane / lease).
        let dup_json = r#"{"version":1,"workspace_id":"ws-dup","revision":1,"windows":[{"window_key":"main","kind":"main","layout":{"node":"split","direction":"horizontal","children":[{"node":"leaf","panel":{"panel_type":"terminal","panel_id":"dup","params":null}},{"node":"leaf","panel":{"panel_type":"terminal","panel_id":"dup","params":null}}]}}]}"#;
        {
            let conn = db.lock();
            conn.execute(
                "INSERT INTO layout_config (workspace_id, version, json, revision) VALUES ('ws-dup', 1, ?1, 1)",
                rusqlite::params![dup_json],
            )
            .unwrap();
        }
        let got = get(&db, "ws-dup").unwrap();
        // Fallback: Main vacío, NO el árbol roto.
        assert_eq!(got.version, CURRENT_VERSION);
        assert_eq!(got.windows.len(), 1);
        assert_eq!(got.windows[0].kind, WindowKind::Main);
        match &got.windows[0].layout {
            PanelLayoutNode::Split { children, .. } => {
                assert!(children.is_empty(), "fallback = workspace vacío")
            }
            _ => panic!("fallback debe ser un Split vacío"),
        }

        // 2) version FUTURA → también fallback.
        let future_json = r#"{"version":999,"workspace_id":"ws-fut","revision":1,"windows":[{"window_key":"main","kind":"main","layout":{"node":"split","direction":"horizontal","children":[]}}]}"#;
        {
            let conn = db.lock();
            conn.execute(
                "INSERT INTO layout_config (workspace_id, version, json, revision) VALUES ('ws-fut', 999, ?1, 1)",
                rusqlite::params![future_json],
            )
            .unwrap();
        }
        let got_fut = get(&db, "ws-fut").unwrap();
        assert_eq!(
            got_fut.version, CURRENT_VERSION,
            "version futura → fallback v1"
        );
        assert_eq!(got_fut.windows.len(), 1);
    }

    #[test]
    fn display_hint_is_optional_and_partial() {
        // un hint parcial (sólo monitor_id) es válido.
        let w = WindowLayout {
            window_key: "main".to_string(),
            kind: WindowKind::Main,
            display_hint: Some(DisplayHint {
                monitor_id: Some("dp-1".to_string()),
                x: None,
                y: None,
                width: None,
                height: None,
            }),
            layout: PanelLayoutNode::Split {
                direction: SplitDirection::Horizontal,
                children: vec![],
            },
        };
        let json = serde_json::to_string(&w).unwrap();
        let back: WindowLayout = serde_json::from_str(&json).unwrap();
        assert_eq!(w, back);
        // campos None se omiten del json (skip_serializing_if).
        assert!(!json.contains("\"x\""));
        assert!(json.contains("monitor_id"));
    }

    // ── T062 — Validador de árbol (una falla por regla) ──────────────────────────

    fn leaf(id: &str) -> PanelLayoutNode {
        PanelLayoutNode::Leaf {
            panel: PanelDescriptor {
                panel_type: "terminal".to_string(),
                panel_id: id.to_string(),
                params: Value::Null,
            },
        }
    }

    /// Config mínima válida: 1 Main con un Split de 2 hojas, revision 1.
    fn valid_cfg() -> LayoutConfigV1 {
        LayoutConfigV1 {
            version: CURRENT_VERSION,
            workspace_id: "ws".to_string(),
            revision: 1,
            windows: vec![WindowLayout {
                window_key: MAIN_WINDOW_KEY.to_string(),
                kind: WindowKind::Main,
                display_hint: None,
                layout: PanelLayoutNode::Split {
                    direction: SplitDirection::Horizontal,
                    children: vec![leaf("a"), leaf("b")],
                },
            }],
        }
    }

    #[test]
    fn validate_accepts_valid_empty_and_migrated() {
        // empty() (split raíz vacío) y migrate (split raíz de 1) son válidos.
        assert!(LayoutConfigV1::empty("ws").validate().is_ok());
        let m = migrate_v0_to_v1("ws", &serde_json::json!([{"id":"p1","mode":"zsh"}]));
        assert!(m.validate().is_ok());
        assert!(valid_cfg().validate().is_ok());
    }

    #[test]
    fn validate_rejects_duplicate_panel_id() {
        let mut cfg = valid_cfg();
        // dos hojas con el MISMO panel_id (cross-tree).
        cfg.windows[0].layout = PanelLayoutNode::Split {
            direction: SplitDirection::Horizontal,
            children: vec![leaf("dup"), leaf("dup")],
        };
        let err = cfg.validate().unwrap_err().to_string();
        assert!(err.contains("panel_id duplicado"), "got: {err}");
    }

    #[test]
    fn validate_rejects_empty_tabs() {
        let mut cfg = valid_cfg();
        cfg.windows[0].layout = PanelLayoutNode::Tabs {
            active: 0,
            children: vec![],
        };
        assert!(cfg
            .validate()
            .unwrap_err()
            .to_string()
            .contains("Tabs vacío"));
    }

    #[test]
    fn validate_rejects_nested_split_with_one_child() {
        let mut cfg = valid_cfg();
        // Split anidado (no raíz) con 1 hijo → inválido.
        cfg.windows[0].layout = PanelLayoutNode::Split {
            direction: SplitDirection::Horizontal,
            children: vec![
                leaf("a"),
                PanelLayoutNode::Split {
                    direction: SplitDirection::Vertical,
                    children: vec![leaf("b")], // <2
                },
            ],
        };
        assert!(cfg
            .validate()
            .unwrap_err()
            .to_string()
            .contains("Split con 1"));
    }

    #[test]
    fn validate_rejects_tabs_active_out_of_range() {
        let mut cfg = valid_cfg();
        cfg.windows[0].layout = PanelLayoutNode::Tabs {
            active: 5,
            children: vec![leaf("a"), leaf("b")],
        };
        assert!(cfg
            .validate()
            .unwrap_err()
            .to_string()
            .contains("fuera de rango"));
    }

    #[test]
    fn validate_rejects_unknown_window_refs_and_dupes() {
        // window_key duplicado.
        let mut cfg = valid_cfg();
        cfg.windows.push(WindowLayout {
            window_key: MAIN_WINDOW_KEY.to_string(), // duplicado
            kind: WindowKind::Detached,
            display_hint: None,
            layout: leaf("c"),
        });
        assert!(cfg
            .validate()
            .unwrap_err()
            .to_string()
            .contains("window_key duplicado"));

        // más de una Main.
        let mut cfg2 = valid_cfg();
        cfg2.windows.push(WindowLayout {
            window_key: "w2".to_string(),
            kind: WindowKind::Main, // 2da Main
            display_hint: None,
            layout: leaf("c"),
        });
        assert!(cfg2
            .validate()
            .unwrap_err()
            .to_string()
            .contains("exactamente 1 ventana Main"));
    }

    #[test]
    fn validate_rejects_unreachable_leaf_detached_empty() {
        // Una ventana detached sin hojas = ventana fantasma (hoja inalcanzable / vacía).
        let mut cfg = valid_cfg();
        cfg.windows.push(WindowLayout {
            window_key: "detached-1".to_string(),
            kind: WindowKind::Detached,
            display_hint: None,
            layout: PanelLayoutNode::Split {
                direction: SplitDirection::Horizontal,
                children: vec![], // 0 hojas
            },
        });
        assert!(cfg
            .validate()
            .unwrap_err()
            .to_string()
            .contains("sin paneles"));
    }

    #[test]
    fn save_rejects_invalid_tree_before_write() {
        // T062: un save inválido NO escribe (la DB queda sin fila).
        let db = test_db();
        let mut cfg = valid_cfg();
        cfg.workspace_id = "ws-x".to_string();
        cfg.windows[0].layout = PanelLayoutNode::Tabs {
            active: 0,
            children: vec![],
        };
        assert!(save(&db, &cfg).is_err());
        let conn = db.lock();
        let n: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM layout_config WHERE workspace_id = ?1",
                rusqlite::params!["ws-x"],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 0, "un save inválido no debe dejar fila");
    }

    // ── T063 — Revisión monotónica (write concurrente: stale rechazado) ──────────

    #[test]
    fn save_requires_monotonic_revision() {
        let db = test_db();
        let mut cfg = valid_cfg();
        cfg.workspace_id = "ws-rev".to_string();
        // primer write: revision debe ser 1 (stored 0 + 1).
        cfg.revision = 1;
        save(&db, &cfg).unwrap();
        // re-save con la MISMA revisión (stale) → rechazado.
        let stale = save(&db, &cfg).unwrap_err().to_string();
        assert!(stale.contains("stale_layout"), "got: {stale}");
        // saltar revisiones (1→3) también es stale (debe ser exactamente +1).
        cfg.revision = 3;
        assert!(save(&db, &cfg)
            .unwrap_err()
            .to_string()
            .contains("stale_layout"));
        // revisión correcta (2) → OK.
        cfg.revision = 2;
        save(&db, &cfg).unwrap();
        let loaded = get(&db, "ws-rev").unwrap();
        assert_eq!(loaded.revision, 2);
    }

    #[test]
    fn concurrent_writers_one_wins_no_corruption() {
        // Dos ventanas leen revision=1 y ambas intentan escribir revision=2. Sólo una gana;
        // la otra recibe stale_layout (y re-leería). El árbol nunca se corrompe.
        let db = test_db();
        let mut base = valid_cfg();
        base.workspace_id = "ws-cc".to_string();
        base.revision = 1;
        save(&db, &base).unwrap();

        // writer A: split distinto, revision 2.
        let mut a = base.clone();
        a.windows[0].layout = PanelLayoutNode::Split {
            direction: SplitDirection::Vertical,
            children: vec![leaf("a1"), leaf("a2")],
        };
        a.revision = 2;
        // writer B: otro split, revision 2 (basado en la MISMA lectura stale).
        let mut b = base.clone();
        b.windows[0].layout = PanelLayoutNode::Split {
            direction: SplitDirection::Horizontal,
            children: vec![leaf("b1"), leaf("b2")],
        };
        b.revision = 2;

        // A escribe primero (gana). B con revision 2 ahora es stale (stored=2 → espera 3).
        save(&db, &a).unwrap();
        let b_err = save(&db, &b).unwrap_err().to_string();
        assert!(
            b_err.contains("stale_layout"),
            "B debe ser rechazado: {b_err}"
        );
        // El árbol persistido es el de A, íntegro.
        let loaded = get(&db, "ws-cc").unwrap();
        assert_eq!(loaded.revision, 2);
        let PanelLayoutNode::Split {
            direction,
            children,
        } = &loaded.windows[0].layout
        else {
            panic!("expected split");
        };
        assert_eq!(*direction, SplitDirection::Vertical);
        assert_eq!(children.len(), 2);
    }
}
