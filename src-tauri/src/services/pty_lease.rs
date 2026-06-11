// services/pty_lease.rs — 018-fase-2-multiwindow-workspace · Phase B0 (council-required)
//
// T060 PtyLeaseRegistry + T061 cierre de ventana TRANSACCIONAL.
//
// INVARIANTE (no convención de front): un `panel_id` se renderiza (y por tanto
// liga su I/O de PTY) en UNA sola webview a la vez. Esta capa vive POR ENCIMA del
// PtyManager (src/pty.rs) y del process_manager (US5). NO mata ni respawnea
// procesos NUNCA: sólo administra el BINDING UI ↔ proceso.
//
// Por qué un registro de leases (council ADAPT, HIGH):
//   - Multi-window (US2) puede montar el MISMO panel_id en 2 webviews (Main +
//     detached, o un reload que re-monta antes de que el viejo se desuscriba) →
//     doble-binding del PTY: dos lectores/escritores de la misma sesión → eco,
//     entrada duplicada, eventos cruzados. El lease lo vuelve IMPOSIBLE: cada
//     attach invalida el lease anterior del panel_id (force-detach versionado).
//   - `mount_instance_id`: cada montaje del componente React genera uno nuevo. Un
//     evento (o un I/O) que trae un mount_instance ya desmontado se DESCARTA — un
//     componente zombie no puede tocar la sesión del montaje vigente.
//
// Relación con `run_token` (pty.rs/process_manager.rs): `run_token` es la
// GENERACIÓN del PROCESO (un respawn del mismo panel_id sube el run_token). El
// `lease_version` de acá es la GENERACIÓN del BINDING UI (un re-attach del mismo
// panel_id sube el lease_version) — ortogonales: el proceso puede vivir intacto
// mientras el binding migra de ventana a ventana.
//
// T061 cierre transaccional: `detach_panel_view` libera SÓLO el binding UI y marca
// `detaching`; NO toca el PTY. Serializa por panel_id (mutex por clave) para que N
// detach/attach concurrentes sobre el mismo panel_id no dejen huérfanos ni un
// binding "running" colgado. El process_manager::detach_viewport (no-op sobre el
// status) sigue siendo el que documenta "el proceso QUEDA".

use parking_lot::Mutex;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

/// Generación monotónica global de leases. Cada `attach_panel` reserva uno; el
/// `lease_version` crece estrictamente → un lease viejo nunca puede "ganarle" a uno
/// nuevo (igual semántica que el `seq` del event bus, aplicada al binding).
static LEASE_SEQ: AtomicU64 = AtomicU64::new(1);

fn next_lease_version() -> u64 {
    LEASE_SEQ.fetch_add(1, Ordering::Relaxed)
}

/// Lease vigente de un panel_id: qué ventana lo tiene ligado, con qué montaje, y la
/// generación del binding. `detaching` = se pidió soltar el binding (cierre/reattach
/// en curso) pero todavía no se confirmó — el proceso NO se toca en ningún caso.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PtyLease {
    pub panel_id: String,
    pub window_label: String,
    pub mount_instance_id: String,
    pub lease_version: u64,
    pub detaching: bool,
}

/// Registro en memoria de leases (uno por panel_id). NO se persiste: es estado de
/// binding UI vigente, no SSOT de layout (eso es LayoutConfigV1). Se reconstruye al
/// re-montar las webviews (cada Leaf hace `attach_panel` al montar).
#[derive(Default)]
pub struct PtyLeaseRegistry {
    /// panel_id → lease vigente. Serializado por la mutex del mapa + (T061) un
    /// mutex por-panel para el crítico detach/attach.
    leases: Mutex<HashMap<String, PtyLease>>,
    /// Mutex por panel_id para serializar el crítico de cada panel (T061). Dos
    /// detach/attach del MISMO panel_id se vuelven secuenciales; panels distintos
    /// no se bloquean entre sí.
    locks: Mutex<HashMap<String, Arc<Mutex<()>>>>,
}

/// Resultado de un `attach_panel`. Si desplazó a un lease anterior (force-detach),
/// `displaced` trae el lease viejo para que el caller pueda avisar a esa ventana
/// (UI) que perdió el binding — SIN tocar el proceso.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttachOutcome {
    pub lease: PtyLease,
    pub displaced: Option<PtyLease>,
}

impl PtyLeaseRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Mutex por-panel (T061). Se crea perezosamente y se reusa.
    fn panel_lock(&self, panel_id: &str) -> Arc<Mutex<()>> {
        let mut locks = self.locks.lock();
        locks
            .entry(panel_id.to_string())
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone()
    }

    /// ATTACH: liga `panel_id` a (`window_label`, `mount_instance_id`). Si ya hay un
    /// lease activo para ese panel_id, lo FUERZA-DETACH (versionado): el nuevo lease
    /// tiene `lease_version` mayor y el viejo queda `displaced`. NUNCA toca el PTY.
    /// Serializado por panel_id.
    pub fn attach_panel(
        &self,
        panel_id: &str,
        window_label: &str,
        mount_instance_id: &str,
    ) -> AttachOutcome {
        let lock = self.panel_lock(panel_id);
        let _guard = lock.lock();
        let mut leases = self.leases.lock();
        let displaced = leases.get(panel_id).cloned();
        let lease = PtyLease {
            panel_id: panel_id.to_string(),
            window_label: window_label.to_string(),
            mount_instance_id: mount_instance_id.to_string(),
            lease_version: next_lease_version(),
            detaching: false,
        };
        leases.insert(panel_id.to_string(), lease.clone());
        AttachOutcome { lease, displaced }
    }

    /// T061 — libera SÓLO el binding UI del panel_id si el lease vigente coincide con
    /// (`window_label`, `mount_instance_id`). Idempotente y serializado por panel_id:
    /// N llamadas concurrentes (X del SO + unmount de React + reattach) no compiten —
    /// la primera que matchea remueve el lease; el resto es no-op. NUNCA toca el PTY.
    /// Devuelve `true` si removió un lease propio.
    ///
    /// Remueve el lease ATÓMICAMENTE dentro del crítico por-panel (no marca `detaching`
    /// como paso intermedio: el remove en sí, bajo el lock, es la transición). El paso
    /// `detaching` previo es OPCIONAL y lo provee `mark_detaching` cuando el caller quiere
    /// cortar el I/O antes del remove final (cierre transaccional en dos fases).
    pub fn detach_panel_view(
        &self,
        panel_id: &str,
        window_label: &str,
        mount_instance_id: &str,
    ) -> bool {
        let lock = self.panel_lock(panel_id);
        let removed = {
            let _guard = lock.lock();
            let mut leases = self.leases.lock();
            match leases.get(panel_id) {
                Some(l)
                    if l.window_label == window_label
                        && l.mount_instance_id == mount_instance_id =>
                {
                    leases.remove(panel_id);
                    true
                }
                // El lease vigente es de OTRA ventana/montaje (un re-attach ya ganó) o no
                // hay lease → no-op. El proceso queda intacto en ambos casos.
                _ => false,
            }
        };
        // _guard liberado acá: ya no contamos ese Arc clone vivo.
        drop(lock);
        // (audit should-fix) Prune del mapa de locks para que no crezca sin límite a lo
        // largo de la sesión (cada panel_id visto deja una entrada). Sólo podamos si: (a) no
        // hay lease para el panel y (b) NADIE más tiene el Arc del lock vivo (strong_count==1
        // ⇒ sólo el mapa lo retiene) → ningún detach/attach concurrente lo está usando. Si
        // alguien lo agarró entre medio, lo dejamos (se podará la próxima). Nunca toca el PTY.
        self.maybe_prune_lock(panel_id);
        removed
    }

    /// Poda la entrada de `locks` de un panel si está inactiva. Conservador: sólo borra
    /// cuando no hay lease y el `Arc<Mutex<()>>` no tiene otros dueños (`strong_count==1`).
    fn maybe_prune_lock(&self, panel_id: &str) {
        if self.leases.lock().contains_key(panel_id) {
            return; // aún hay binding → el lock sigue siendo útil.
        }
        let mut locks = self.locks.lock();
        if let Some(arc) = locks.get(panel_id) {
            if Arc::strong_count(arc) == 1 {
                locks.remove(panel_id);
            }
        }
    }

    /// Lease vigente de un panel_id (clone), o `None`.
    pub fn current_lease(&self, panel_id: &str) -> Option<PtyLease> {
        self.leases.lock().get(panel_id).cloned()
    }

    /// ¿EXISTE un lease (vigente o `detaching`) para este `panel_id`, sin importar qué
    /// ventana/montaje lo tiene? (HIGH-1 audit) — usado por `pty_write` para cerrar el
    /// fail-open: si un panel TIENE binding bajo el registro de leases, un write que NO
    /// declara su (window_label, mount_instance_id) se DESCARTA (fail-CLOSED), porque sólo
    /// el binding vigente puede escribir. Si NO hay lease (caller legacy, flag OFF), el
    /// write sigue el camino fail-open. NUNCA toca el proceso PTY.
    pub fn has_lease(&self, panel_id: &str) -> bool {
        self.leases.lock().contains_key(panel_id)
    }

    /// GUARD anti-evento-stale (T060): ¿el evento que viene de `mount_instance_id` en
    /// `window_label` corresponde al lease VIGENTE del panel_id? Un componente ya
    /// desmontado (mount_instance viejo) o una ventana que perdió el binding (otra
    /// ganó) devuelve `false` → su I/O / su evento se DESCARTA. Sin esto, un pane
    /// zombie escribiría/leería sobre la sesión del montaje vigente.
    pub fn is_current(&self, panel_id: &str, window_label: &str, mount_instance_id: &str) -> bool {
        match self.leases.lock().get(panel_id) {
            Some(l) => {
                l.window_label == window_label
                    && l.mount_instance_id == mount_instance_id
                    && !l.detaching
            }
            None => false,
        }
    }

    /// Marca el lease del panel_id como `detaching` (paso 1 del cierre transaccional,
    /// T061) si coincide con el montaje dado. El proceso NO se toca. Devuelve `true`
    /// si marcó. Tras esto `is_current` devuelve `false` (no más I/O por ese binding)
    /// pero el lease sigue presente hasta el `detach_panel_view` final.
    pub fn mark_detaching(
        &self,
        panel_id: &str,
        window_label: &str,
        mount_instance_id: &str,
    ) -> bool {
        let lock = self.panel_lock(panel_id);
        let _guard = lock.lock();
        let mut leases = self.leases.lock();
        if let Some(l) = leases.get_mut(panel_id) {
            if l.window_label == window_label && l.mount_instance_id == mount_instance_id {
                l.detaching = true;
                return true;
            }
        }
        false
    }

    /// Todos los panel_id ligados a una ventana (para el cierre de ventana: reatar a
    /// Main y soltar sus bindings). Clone barato del set vigente.
    pub fn panels_for_window(&self, window_label: &str) -> Vec<String> {
        self.leases
            .lock()
            .values()
            .filter(|l| l.window_label == window_label)
            .map(|l| l.panel_id.clone())
            .collect()
    }

    /// Cantidad de leases vivos (debug/tests).
    pub fn len(&self) -> usize {
        self.leases.lock().len()
    }

    pub fn is_empty(&self) -> bool {
        self.leases.lock().is_empty()
    }

    /// (tests) Cantidad de entradas en el mapa de locks por-panel — para verificar el prune.
    #[cfg(test)]
    fn locks_len(&self) -> usize {
        self.locks.lock().len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;

    #[test]
    fn lease_is_unique_per_panel_id() {
        // Un panel_id tiene a lo sumo UN lease vigente; un segundo attach (otra ventana)
        // desplaza al primero (force-detach versionado), no acumula.
        let reg = PtyLeaseRegistry::new();
        let a = reg.attach_panel("p1", "main", "m1");
        assert!(a.displaced.is_none());
        assert_eq!(reg.len(), 1);

        let b = reg.attach_panel("p1", "detached-1", "m2");
        // El nuevo lease desplazó al viejo (mismo panel_id, otra ventana).
        assert_eq!(b.displaced.as_ref().unwrap().window_label, "main");
        assert!(
            b.lease.lease_version > a.lease.lease_version,
            "force-detach versionado"
        );
        assert_eq!(reg.len(), 1, "sigue habiendo UN solo lease para p1");
        // El binding vigente es el de la ventana detached.
        assert_eq!(reg.current_lease("p1").unwrap().window_label, "detached-1");
    }

    #[test]
    fn force_detach_is_versioned_monotonic() {
        // Cada attach del mismo panel_id sube estrictamente el lease_version.
        let reg = PtyLeaseRegistry::new();
        let v1 = reg.attach_panel("p", "w", "m1").lease.lease_version;
        let v2 = reg.attach_panel("p", "w", "m2").lease.lease_version;
        let v3 = reg.attach_panel("p", "w", "m3").lease.lease_version;
        assert!(v1 < v2 && v2 < v3);
    }

    #[test]
    fn stale_mount_instance_event_is_discarded() {
        // Un evento/IO que trae un mount_instance ya reemplazado se descarta (is_current=false).
        let reg = PtyLeaseRegistry::new();
        reg.attach_panel("p1", "main", "mount-A");
        assert!(reg.is_current("p1", "main", "mount-A"));
        // El componente remonta (mismo panel, mismo window) con un mount nuevo.
        reg.attach_panel("p1", "main", "mount-B");
        // El evento del montaje viejo (A) YA NO es vigente → se descarta.
        assert!(!reg.is_current("p1", "main", "mount-A"));
        assert!(reg.is_current("p1", "main", "mount-B"));
        // Otra ventana tampoco es vigente.
        assert!(!reg.is_current("p1", "detached-1", "mount-B"));
        // Un panel sin lease nunca es vigente.
        assert!(!reg.is_current("desconocido", "main", "x"));
    }

    #[test]
    fn detach_panel_view_only_releases_matching_binding() {
        let reg = PtyLeaseRegistry::new();
        reg.attach_panel("p1", "main", "m1");
        // Detach de OTRA ventana/montaje → no-op (no le pertenece).
        assert!(!reg.detach_panel_view("p1", "detached-1", "m1"));
        assert!(!reg.detach_panel_view("p1", "main", "m-otro"));
        assert_eq!(reg.len(), 1);
        // Detach del binding correcto → libera (sólo UI).
        assert!(reg.detach_panel_view("p1", "main", "m1"));
        assert_eq!(reg.len(), 0);
        // Idempotente: un segundo detach es no-op.
        assert!(!reg.detach_panel_view("p1", "main", "m1"));
    }

    #[test]
    fn mark_detaching_stops_current_without_removing() {
        let reg = PtyLeaseRegistry::new();
        reg.attach_panel("p1", "main", "m1");
        assert!(reg.is_current("p1", "main", "m1"));
        assert!(reg.mark_detaching("p1", "main", "m1"));
        // Marcado detaching: ya no es vigente para I/O, pero el lease sigue (proceso intacto).
        assert!(!reg.is_current("p1", "main", "m1"));
        assert_eq!(reg.len(), 1);
        assert!(reg.current_lease("p1").unwrap().detaching);
    }

    #[test]
    fn panels_for_window_lists_bindings() {
        let reg = PtyLeaseRegistry::new();
        reg.attach_panel("p1", "main", "m1");
        reg.attach_panel("p2", "main", "m2");
        reg.attach_panel("p3", "detached-1", "m3");
        let mut main_panels = reg.panels_for_window("main");
        main_panels.sort();
        assert_eq!(main_panels, vec!["p1".to_string(), "p2".to_string()]);
        assert_eq!(reg.panels_for_window("detached-1"), vec!["p3".to_string()]);
        assert!(reg.panels_for_window("inexistente").is_empty());
    }

    #[test]
    fn concurrent_detach_attach_same_panel_no_orphans() {
        // T061 test de concurrencia: N hilos hacen attach+detach del MISMO panel_id en
        // simultáneo. El crítico serializado por panel_id garantiza que NO queda un
        // binding inconsistente (el registro converge a 0 o 1 lease, nunca corrupto),
        // y en NINGÚN momento se toca un proceso (esta capa no tiene acceso al PTY).
        let reg = Arc::new(PtyLeaseRegistry::new());
        let mut handles = Vec::new();
        for i in 0..32 {
            let reg = Arc::clone(&reg);
            handles.push(thread::spawn(move || {
                let mount = format!("m{i}");
                let out = reg.attach_panel("shared", "main", &mount);
                // Inmediatamente intenta soltar SU propio binding. Si otro hilo ya ganó
                // el lease, el detach es no-op (no rompe nada, no toca proceso).
                let _ = out;
                reg.detach_panel_view("shared", "main", &mount);
            }));
        }
        for h in handles {
            h.join().unwrap();
        }
        // Invariante: como mucho UN lease vigente (el del último attach que no fue
        // soltado por su propio hilo); jamás un estado corrupto. 0 ó 1, nunca >1.
        assert!(
            reg.len() <= 1,
            "a lo sumo un lease vigente, got {}",
            reg.len()
        );
    }

    /// HIGH-1 (audit) — reproduce la decisión del guard de `pty_write` a nivel registro:
    /// con un lease vigente, un write SIN params (o con params equivocados) se DESCARTA;
    /// sólo el binding vigente (params correctos) escribe. El "descarte" se modela como la
    /// misma rama que `pty_write` toma para devolver `Ok(())` sin tocar el PTY.
    #[test]
    fn write_guard_fail_closed_when_lease_exists_without_params() {
        let reg = PtyLeaseRegistry::new();
        reg.attach_panel("p1", "main", "m1");

        // Caller SIN params → el guard mira has_lease. Hay lease ⇒ DESCARTAR (fail-closed).
        assert!(
            reg.has_lease("p1"),
            "el panel tiene lease ⇒ write sin params se descarta"
        );

        // Caller con params CORRECTOS (binding vigente) → is_current=true ⇒ ESCRIBE.
        assert!(
            reg.is_current("p1", "main", "m1"),
            "binding vigente ⇒ write procede"
        );

        // Caller con params de OTRO montaje (stale) → is_current=false ⇒ DESCARTAR.
        assert!(
            !reg.is_current("p1", "main", "m-stale"),
            "binding stale ⇒ write se descarta"
        );
    }

    /// HIGH-1 (audit) — sin lease (caller legacy / flag OFF) el guard hace fail-OPEN:
    /// has_lease=false ⇒ el write procede (camino legacy intacto).
    #[test]
    fn write_guard_fail_open_when_no_lease() {
        let reg = PtyLeaseRegistry::new();
        // Nunca se hizo attach para "legacy-pane" → no hay lease.
        assert!(
            !reg.has_lease("legacy-pane"),
            "sin lease ⇒ fail-open (legacy escribe normal)"
        );
        // Y si se suelta el binding, vuelve a fail-open.
        reg.attach_panel("p2", "main", "m1");
        assert!(reg.has_lease("p2"));
        reg.detach_panel_view("p2", "main", "m1");
        assert!(
            !reg.has_lease("p2"),
            "tras detach el panel ya no tiene lease ⇒ fail-open"
        );
    }

    /// `has_lease` reconoce un lease aunque esté `detaching` (binding aún presente).
    #[test]
    fn has_lease_true_even_while_detaching() {
        let reg = PtyLeaseRegistry::new();
        reg.attach_panel("p1", "main", "m1");
        reg.mark_detaching("p1", "main", "m1");
        // detaching ⇒ NO vigente para I/O…
        assert!(!reg.is_current("p1", "main", "m1"));
        // …pero el lease sigue presente ⇒ has_lease=true ⇒ un write sin params igual se descarta
        // (no se cuela por la ventana de cierre transaccional).
        assert!(reg.has_lease("p1"));
    }

    /// (audit should-fix) El mapa de locks se PODA cuando un panel queda sin lease: un
    /// attach+detach no debe dejar una entrada de lock viva para siempre.
    #[test]
    fn detach_prunes_idle_panel_lock() {
        let reg = PtyLeaseRegistry::new();
        reg.attach_panel("p1", "main", "m1");
        assert_eq!(reg.locks_len(), 1, "el attach creó el lock del panel");
        reg.detach_panel_view("p1", "main", "m1");
        assert_eq!(reg.len(), 0, "lease liberado");
        assert_eq!(
            reg.locks_len(),
            0,
            "lock podado al quedar el panel sin binding"
        );

        // Si el panel sigue teniendo lease (detach de otro montaje no-matcheante), NO se poda.
        reg.attach_panel("p2", "main", "m1");
        reg.detach_panel_view("p2", "main", "m-otro"); // no-op (no matchea)
        assert_eq!(reg.len(), 1, "p2 sigue ligado");
        assert_eq!(
            reg.locks_len(),
            1,
            "lock de p2 NO se poda porque sigue habiendo lease"
        );
    }

    #[test]
    fn distinct_panels_do_not_block_each_other() {
        // Panels distintos usan locks distintos → no hay serialización cruzada.
        let reg = Arc::new(PtyLeaseRegistry::new());
        let mut handles = Vec::new();
        for i in 0..16 {
            let reg = Arc::clone(&reg);
            handles.push(thread::spawn(move || {
                let panel = format!("p{i}");
                reg.attach_panel(&panel, "main", "m");
            }));
        }
        for h in handles {
            h.join().unwrap();
        }
        assert_eq!(reg.len(), 16, "16 panels distintos → 16 leases");
    }
}
