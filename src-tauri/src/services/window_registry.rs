// services/window_registry.rs — 018-fase-2-multiwindow-workspace · US2 (T020)
//
// REGISTRO RUNTIME de ventanas: label ↔ window_key ↔ panel_ids. Es el dueño del
// ciclo de vida de las ventanas detached a nivel de PROCESO de la app (no del SO):
// alta al abrir (`window_open_detached`), baja al cerrar (`window_close` / X del SO),
// ruteo de foco/eventos, y CLEANUP — SIN matar procesos (constitución VI).
//
// Relación con las otras capas (todas ortogonales):
//   - `LayoutConfigV1` (layout_config.rs) = SSOT PERSISTIDO del árbol/ventanas (DB).
//     Este registro NO lo reemplaza: es el espejo EN MEMORIA del estado de las
//     WebviewWindow vivas (un handle de OS no se persiste). El árbol persiste qué
//     ventanas EXISTEN; el registro sabe cuáles están ABIERTAS ahora mismo.
//   - `PtyLeaseRegistry` (pty_lease.rs) = binding UI↔PTY por panel_id. El registro de
//     ventanas usa `panels_for_window` del lease como fuente de verdad de qué panes
//     tiene una ventana al cerrarla (los Leaf hacen attach/detach al montar/desmontar).
//   - `window_reattach.rs` = transformación PURA del árbol al cerrar (reatar a Main).
//
// INVARIANTE: la ventana Main (`MAIN_WINDOW_KEY`) NUNCA se da de baja del registro por
// esta capa (su ciclo lo maneja el `on_window_event` global de la app). Sólo las
// detached entran/salen. Cerrar una detached: baja del registro + reatar su layout a
// Main (caller) — JAMÁS un `pty_kill`.
//
// `window_key` vs `label`: en US2 los hacemos COINCIDIR (el label de la WebviewWindow
// se deriva 1:1 del window_key estable, p.ej. "detached-1"). El registro guarda ambos
// para no asumir esa igualdad en el futuro (multi-workspace podría romperla).

use parking_lot::Mutex;
use std::collections::HashMap;

use crate::services::layout_config::MAIN_WINDOW_KEY;

/// Una ventana registrada (viva en el proceso). `panel_ids` es el último conjunto
/// conocido de panes que renderiza — se refresca desde el lease al cerrar/rutear.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowEntry {
    /// Label real de la `WebviewWindow` de Tauri (handle de UI). Único.
    pub label: String,
    /// Clave ESTABLE de layout (window_key del `LayoutConfigV1`). En US2 == label.
    pub window_key: String,
    /// ¿Es la ventana Main? (su ciclo lo maneja la app, no esta capa).
    pub is_main: bool,
}

/// Registro en memoria de las ventanas vivas. NO se persiste (el SSOT del árbol es
/// `LayoutConfigV1`; este registro es el espejo de los handles de OS abiertos).
#[derive(Default)]
pub struct WindowRegistry {
    /// label → entry. La Main se registra una vez al boot; las detached entran/salen.
    by_label: Mutex<HashMap<String, WindowEntry>>,
    /// Labels cuyo cierre transaccional YA fue procesado (reatado a Main hecho) y que sólo
    /// esperan que la `WebviewWindow` termine de cerrarse. El listener `onCloseRequested`
    /// (T022) usa esto para NO re-prevenir/re-reatar un cierre re-entrante: la PRIMERA vez
    /// previene + reata + marca acá; el `w.close()` dispara otro `onCloseRequested` que, al
    /// ver el label marcado, deja cerrar sin re-procesar. Evita doble-reattach y loops.
    settling: Mutex<std::collections::HashSet<String>>,
}

/// Resultado de dar de baja una ventana. `panel_ids` son los panes que tenía ligados
/// (vía el lease) en el momento de cerrar — el caller los reata a Main. NUNCA se matan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CloseOutcome {
    pub window_key: String,
    /// `true` si la ventana existía y se removió; `false` si no estaba (idempotente).
    pub removed: bool,
    /// `true` si era la Main (no se remueve por esta capa; el caller no debe reatar).
    pub was_main: bool,
}

impl WindowRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Registra la ventana Main (idempotente). El label de la Main de Tauri es "main"
    /// (== `MAIN_WINDOW_KEY`). Llamado una vez en el boot.
    pub fn register_main(&self) {
        let mut m = self.by_label.lock();
        m.entry(MAIN_WINDOW_KEY.to_string())
            .or_insert_with(|| WindowEntry {
                label: MAIN_WINDOW_KEY.to_string(),
                window_key: MAIN_WINDOW_KEY.to_string(),
                is_main: true,
            });
    }

    /// ALTA de una ventana detached. Idempotente: si ya estaba (mismo label), no
    /// duplica (devuelve `false`). `window_key` y `label` coinciden en US2.
    pub fn register_detached(&self, label: &str, window_key: &str) -> bool {
        let mut m = self.by_label.lock();
        if m.contains_key(label) {
            return false;
        }
        m.insert(
            label.to_string(),
            WindowEntry {
                label: label.to_string(),
                window_key: window_key.to_string(),
                is_main: false,
            },
        );
        true
    }

    /// BAJA de una ventana (cierre). Idempotente. La Main NO se remueve por esta capa
    /// (`was_main: true`, `removed: false`) — su ciclo lo maneja la app. Para una
    /// detached, la remueve y devuelve su `window_key` para que el caller reate su
    /// subárbol a Main (vía `window_reattach::reattach_window_to_main`). NUNCA toca el PTY.
    pub fn close(&self, label: &str) -> Option<CloseOutcome> {
        let mut m = self.by_label.lock();
        let Some(entry) = m.get(label).cloned() else {
            // No estaba registrada → idempotente, no-op.
            return None;
        };
        if entry.is_main {
            // La Main no se da de baja acá (cerrar Main = cerrar todo, política aparte).
            return Some(CloseOutcome {
                window_key: entry.window_key,
                removed: false,
                was_main: true,
            });
        }
        m.remove(label);
        Some(CloseOutcome {
            window_key: entry.window_key,
            removed: true,
            was_main: false,
        })
    }

    /// Lookup del `window_key` de una ventana por su label (ruteo).
    pub fn window_key_for(&self, label: &str) -> Option<String> {
        self.by_label
            .lock()
            .get(label)
            .map(|e| e.window_key.clone())
    }

    /// ¿Está registrada una ventana con este label?
    pub fn contains(&self, label: &str) -> bool {
        self.by_label.lock().contains_key(label)
    }

    /// ¿Es una ventana detached VIVA (registrada y NO-Main)? PEEK sin remover — el settle
    /// transaccional (018 US2 audit) lo usa para decidir reatar ANTES de tocar el registro:
    /// así, si la persistencia del reattach falla, la ventana sigue registrada y un retry
    /// puede reatar sus panes (no quedan PTY huérfanos).
    pub fn is_live_detached(&self, label: &str) -> bool {
        self.by_label
            .lock()
            .get(label)
            .map(|e| !e.is_main)
            .unwrap_or(false)
    }

    /// Snapshot de todas las ventanas registradas (para `window_list` / debug). Orden
    /// determinista (por label) para que el front no dependa del orden del HashMap.
    pub fn list(&self) -> Vec<WindowEntry> {
        let mut v: Vec<WindowEntry> = self.by_label.lock().values().cloned().collect();
        v.sort_by(|a, b| a.label.cmp(&b.label));
        v
    }

    /// Cantidad de ventanas registradas (debug/tests).
    pub fn len(&self) -> usize {
        self.by_label.lock().len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_label.lock().is_empty()
    }

    /// T022 — marca un label como "settling" (cierre transaccional ya procesado). Devuelve
    /// `true` si lo marcó AHORA (era la primera vez), `false` si ya estaba marcado (cierre
    /// re-entrante). El listener `onCloseRequested` usa el resultado: primera vez → prevenir +
    /// reatar; ya marcado → dejar cerrar sin re-procesar.
    pub fn begin_settle(&self, label: &str) -> bool {
        self.settling.lock().insert(label.to_string())
    }

    /// ¿está este label en proceso de cierre ya procesado?
    pub fn is_settling(&self, label: &str) -> bool {
        self.settling.lock().contains(label)
    }

    /// Limpia la marca de settling (tras cerrarse efectivamente la ventana).
    pub fn end_settle(&self, label: &str) {
        self.settling.lock().remove(label);
    }

    /// Labels de TODAS las ventanas detached registradas (para cerrar todo al cerrar Main,
    /// sin huérfanos). NO incluye la Main.
    pub fn detached_labels(&self) -> Vec<String> {
        let mut v: Vec<String> = self
            .by_label
            .lock()
            .values()
            .filter(|e| !e.is_main)
            .map(|e| e.label.clone())
            .collect();
        v.sort();
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn main_registers_once_idempotent() {
        let reg = WindowRegistry::new();
        reg.register_main();
        reg.register_main(); // idempotente
        assert_eq!(reg.len(), 1);
        assert!(reg.contains(MAIN_WINDOW_KEY));
        assert_eq!(
            reg.window_key_for(MAIN_WINDOW_KEY).as_deref(),
            Some(MAIN_WINDOW_KEY)
        );
    }

    #[test]
    fn detached_alta_y_baja_sin_tocar_procesos() {
        let reg = WindowRegistry::new();
        reg.register_main();
        // ALTA.
        assert!(reg.register_detached("detached-1", "detached-1"));
        // Idempotente: segunda alta del mismo label → false (no duplica).
        assert!(!reg.register_detached("detached-1", "detached-1"));
        assert_eq!(reg.len(), 2);
        assert!(reg.contains("detached-1"));

        // BAJA (cierre): remueve la detached, devuelve su window_key para reatar.
        let out = reg.close("detached-1").expect("estaba registrada");
        assert!(out.removed);
        assert!(!out.was_main);
        assert_eq!(out.window_key, "detached-1");
        assert_eq!(reg.len(), 1, "sólo queda Main tras cerrar la detached");
        assert!(!reg.contains("detached-1"));
        // Esta capa NO tiene acceso al PTY → por construcción no puede matar procesos.
    }

    #[test]
    fn is_live_detached_peeks_without_removing() {
        // 018 US2 audit (#1): el settle transaccional PEEKea con is_live_detached ANTES de
        // remover, para poder reatar y, si la persistencia falla, dejar la ventana registrada
        // (retry sin huérfanos). El peek NO debe mutar el registro.
        let reg = WindowRegistry::new();
        reg.register_main();
        reg.register_detached("detached-1", "detached-1");
        assert!(reg.is_live_detached("detached-1"));
        assert!(
            !reg.is_live_detached(MAIN_WINDOW_KEY),
            "Main no es detached"
        );
        assert!(!reg.is_live_detached("no-existe"));
        // El peek no removió nada.
        assert_eq!(reg.len(), 2);
        assert!(reg.contains("detached-1"));
    }

    #[test]
    fn settling_mark_lifecycle() {
        // 018 US2 audit (#3): la marca settling debe poder liberarse en caminos no-exitosos
        // (settle no-op / error) para que un `detached-N` reusado no quede stale y saltee el
        // reatado en su próximo cierre.
        let reg = WindowRegistry::new();
        assert!(reg.begin_settle("detached-1"), "primer begin → true");
        assert!(reg.is_settling("detached-1"));
        assert!(
            !reg.begin_settle("detached-1"),
            "segundo begin → false (ya marcado)"
        );
        reg.end_settle("detached-1");
        assert!(
            !reg.is_settling("detached-1"),
            "tras end_settle ya no está settling"
        );
        // Reuso del mismo label: begin vuelve a tomar la marca limpiamente.
        assert!(
            reg.begin_settle("detached-1"),
            "label reusado → begin vuelve a true"
        );
    }

    #[test]
    fn closing_unregistered_is_noop() {
        let reg = WindowRegistry::new();
        reg.register_main();
        assert!(
            reg.close("no-existe").is_none(),
            "cerrar una ventana no registrada = no-op"
        );
        assert_eq!(reg.len(), 1);
    }

    #[test]
    fn closing_main_does_not_remove_it() {
        // Cerrar la Main por esta capa NO la remueve (su ciclo lo maneja la app); se
        // marca was_main para que el caller NO intente reatar (cerrar Main = cerrar todo).
        let reg = WindowRegistry::new();
        reg.register_main();
        let out = reg.close(MAIN_WINDOW_KEY).expect("Main registrada");
        assert!(out.was_main);
        assert!(!out.removed);
        assert!(reg.contains(MAIN_WINDOW_KEY), "Main sigue registrada");
    }

    #[test]
    fn list_is_deterministic_and_detached_labels_excludes_main() {
        let reg = WindowRegistry::new();
        reg.register_main();
        reg.register_detached("detached-2", "detached-2");
        reg.register_detached("detached-1", "detached-1");
        // list() ordenado por label.
        let labels: Vec<String> = reg.list().into_iter().map(|e| e.label).collect();
        assert_eq!(labels, vec!["detached-1", "detached-2", "main"]);
        // detached_labels excluye Main, ordenado.
        assert_eq!(
            reg.detached_labels(),
            vec!["detached-1".to_string(), "detached-2".to_string()]
        );
    }

    #[test]
    fn no_orphans_after_close_all_detached() {
        // Tras cerrar todas las detached, sólo queda Main (sin huérfanos en el registro).
        let reg = WindowRegistry::new();
        reg.register_main();
        for i in 1..=4 {
            reg.register_detached(&format!("detached-{i}"), &format!("detached-{i}"));
        }
        assert_eq!(reg.len(), 5);
        for l in reg.detached_labels() {
            let out = reg.close(&l).unwrap();
            assert!(out.removed);
        }
        assert_eq!(reg.len(), 1);
        assert_eq!(reg.list()[0].label, MAIN_WINDOW_KEY);
    }
}
