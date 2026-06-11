// services/event_bus.rs — 015-frontend-reform-kernel · US3 (State sync layer / event bus tipado).
//
// Bus de eventos TIPADO Rust → todas las webview windows. El backend Rust es el single source
// of truth (SSOT) del estado operativo CRÍTICO (tareas, sesiones, agentes, approvals, layout,
// comandos). Este módulo NO migra ese estado: define el CONTRATO + el helper de emisión + el
// contador de secuencia monotónico. El estado EFÍMERO (hover, tabs, draft, filtros) NO pasa por
// acá — queda en el front (YAGNI, spec US3).
//
// Garantías del bus:
//   1. Cada evento emitido lleva un envelope { seq, ts, payload } con `seq` MONOTÓNICO global
//      (AtomicU64). El front descarta eventos con seq menor al último visto → un snapshot viejo
//      NUNCA pisa uno nuevo, aunque lleguen fuera de orden / desde 2 viewports.
//   2. Coalescing/diffing: NO se emiten N eventos idénticos consecutivos. Un pequeño debounce
//      POR TIPO de evento evita inundar el IPC (p.ej. progress que cambia 60×/s). Un evento que
//      se coalesce NO consume seq (el seq sólo avanza para lo que realmente sale al IPC).
//
// Wiring real: `emit_event(&app, AppEvent)` emite a TODAS las ventanas vía `app.emit` (Tauri v2).
// Hay un nodo de DEMOSTRACIÓN cableado en `orchestration_set_state` (commands.rs), en paralelo a
// las señales 010 ya existentes, sin tocar su comportamiento.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};
use tokio::sync::broadcast;

/// Nombre del canal Tauri por el que viajan TODOS los eventos del bus. El front escucha este
/// único canal y desmultiplexa por el `tag` del payload (ver web/src/lib/eventBus.ts).
pub const BUS_CHANNEL: &str = "furx:event";

/// Contador de secuencia GLOBAL monotónico. Compartido por todos los emisores del proceso.
/// `fetch_add` es atómico → no hay carrera aunque dos comandos emitan en paralelo.
static SEQ: AtomicU64 = AtomicU64::new(1);

/// Set TIPADO y EXTENSIBLE de eventos del estado crítico. Empezamos chico (spec US3: "set
/// pequeño extensible"). Cada variante es `#[serde(rename_all)]`-friendly y lleva sólo lo
/// mínimo que el front necesita para rehidratar (NO el snapshot entero — eso lo pide por comando).
///
/// Discriminado por `tag` + `data` (serde adjacently tagged) para que el espejo TS sea trivial:
///   { "tag": "TaskChanged", "data": { "id": "...", "state": "running" } }
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "tag", content = "data")]
pub enum AppEvent {
    /// Una tarea de orquestación cambió de estado (008/012/014). El front re-fetchea la tarea.
    TaskChanged { id: String, state: String },
    /// Un agente cambió de estado operativo (idle/running/awaiting_input/…).
    AgentStateChanged { id: String, state: String },
    /// El layout de paneles/ventanas cambió (US6). `window_id` para multi-window-readiness.
    LayoutChanged { window_id: String },
    /// Un comando del registry se ejecutó (US1/US2). Alimenta historial/undo/telemetry.
    CommandExecuted { command_id: String },
    /// Un comando destructivo/credential quedó `pending_approval` (US4 gate).
    ApprovalRequested {
        request_id: String,
        command_id: String,
    },
}

impl AppEvent {
    /// Clave de TIPO usada para el debounce/coalescing por-tipo. Estable y barata.
    fn kind(&self) -> &'static str {
        match self {
            AppEvent::TaskChanged { .. } => "TaskChanged",
            AppEvent::AgentStateChanged { .. } => "AgentStateChanged",
            AppEvent::LayoutChanged { .. } => "LayoutChanged",
            AppEvent::CommandExecuted { .. } => "CommandExecuted",
            AppEvent::ApprovalRequested { .. } => "ApprovalRequested",
        }
    }

    /// Firma de CONTENIDO para diffing. Dos eventos con la misma firma son "idénticos" a efectos
    /// de coalescing. Usamos el JSON canónico del payload (serde produce orden estable de campos).
    fn signature(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }

    /// ¿Este evento puede coalescerse (descartarse si llega un duplicado dentro del debounce)?
    /// Audit codex+deepseek US3: el coalescing por firma podía DESCARTAR eventos SEMÁNTICOS reales
    /// (dos `CommandExecuted` iguales, o un `running→failed→running` que vuelve a la misma firma
    /// dentro de 150ms). Por eso el coalescing es OPT-IN: los eventos semánticos NUNCA se coalescen
    /// (cada ocurrencia importa). Sólo variantes futuras de alta frecuencia e idempotentes
    /// (progress/heartbeat) deberían devolver `true`.
    fn coalescible(&self) -> bool {
        match self {
            AppEvent::TaskChanged { .. }
            | AppEvent::AgentStateChanged { .. }
            | AppEvent::LayoutChanged { .. }
            | AppEvent::CommandExecuted { .. }
            | AppEvent::ApprovalRequested { .. } => false,
        }
    }
}

/// Envelope que viaja por el IPC. `seq` monotónico + `ts` epoch-millis + payload tipado.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventEnvelope {
    pub seq: u64,
    pub ts: i64,
    #[serde(flatten)]
    pub payload: AppEvent,
}

/// Ventana de debounce por defecto: dentro de este lapso, un evento del MISMO tipo con la MISMA
/// firma se coalesce (no se re-emite). Suficientemente corto para no perder transiciones reales,
/// suficientemente largo para absorber ráfagas de progress.
const DEFAULT_DEBOUNCE: Duration = Duration::from_millis(150);

/// Estado del coalescer: última firma + instante de emisión POR TIPO. No global-lock-heavy:
/// un `Mutex<HashMap>` chico, sólo tocado en el path de emisión (no hot-loop puro).
struct Coalescer {
    last: HashMap<&'static str, (String, Instant)>,
    debounce: Duration,
}

impl Coalescer {
    fn new(debounce: Duration) -> Self {
        Self {
            last: HashMap::new(),
            debounce,
        }
    }

    /// Decide si `ev` debe emitirse. Devuelve `false` (skip) si es idéntico al último de su tipo
    /// DENTRO de la ventana de debounce. Si cambia la firma, o pasó el debounce, se emite y se
    /// actualiza el registro. `now` se inyecta para testear sin reloj real.
    fn should_emit(&mut self, ev: &AppEvent, now: Instant) -> bool {
        // Audit US3: los eventos NO-coalescibles (todos los semánticos hoy) siempre se emiten.
        if !ev.coalescible() {
            return true;
        }
        self.note_and_should_emit(ev.kind(), &ev.signature(), now)
    }

    /// Mecanismo de coalescing puro (testeable independiente del gate `coalescible`): mismo
    /// (kind, firma) dentro de la ventana de debounce → coalesce (false); firma distinta o
    /// post-debounce → emite (true) y actualiza el registro.
    fn note_and_should_emit(&mut self, kind: &'static str, sig: &str, now: Instant) -> bool {
        match self.last.get(kind) {
            Some((last_sig, last_at))
                if last_sig == sig && now.duration_since(*last_at) < self.debounce =>
            {
                false
            }
            _ => {
                self.last.insert(kind, (sig.to_string(), now));
                true
            }
        }
    }
}

static COALESCER: once_cell::sync::Lazy<parking_lot::Mutex<Coalescer>> =
    once_cell::sync::Lazy::new(|| parking_lot::Mutex::new(Coalescer::new(DEFAULT_DEBOUNCE)));

/// Reserva el próximo `seq` monotónico. Sólo se llama para eventos que REALMENTE salen al IPC,
/// así el seq no tiene huecos por coalescing (front puede asumir densidad creciente).
fn next_seq() -> u64 {
    SEQ.fetch_add(1, Ordering::SeqCst)
}

/// Núcleo testeable: aplica coalescing + asigna seq. Devuelve el envelope a emitir, o `None` si
/// el evento fue coalescido. NO toca Tauri (testeable sin AppHandle / sin runtime).
pub fn prepare(ev: AppEvent) -> Option<EventEnvelope> {
    prepare_at(ev, Instant::now())
}

/// Igual que `prepare` pero con `now` inyectado (para tests deterministas del debounce).
pub fn prepare_at(ev: AppEvent, now: Instant) -> Option<EventEnvelope> {
    let mut c = COALESCER.lock();
    if !c.should_emit(&ev, now) {
        return None;
    }
    drop(c);
    Some(EventEnvelope {
        seq: next_seq(),
        ts: chrono::Utc::now().timestamp_millis(),
        payload: ev,
    })
}

// ───────────────────── 017 mobile-companion — WS fan-out ─────────────────────
//
// El mobile bridge NO es una webview window: corre un servidor WS y no recibe los
// `app.emit(BUS_CHANNEL)`. Para que el companion consuma el MISMO modelo de eventos
// tipados con el MISMO `seq` (FR-009/FR-010), publicamos cada envelope YA PREPARADO
// (seq asignado, coalescing aplicado) en un broadcast global; el bridge se suscribe
// por conexión y lo reenvía firmado. El seq es idéntico al de las ventanas → orden
// monotónico compartido (un evento viejo nunca pisa uno nuevo, igual que el front).
//
// Mismo patrón que NOTIFY_BUS en mobile_bridge.rs. `send` falla sólo si no hay
// suscriptores (sin móvil conectado) → se ignora. Capacidad 512: un móvil lento
// que se atrasa pierde los más viejos (Lagged) y re-sincroniza por snapshot al
// reconectar (FR-011) — aceptable, el front aplica orden por seq igualmente.
static EVENT_BUS: once_cell::sync::Lazy<broadcast::Sender<EventEnvelope>> =
    once_cell::sync::Lazy::new(|| broadcast::channel(512).0);

/// Suscribirse al fan-out de envelopes (para el mobile bridge). Cada receiver ve
/// los envelopes emitidos DESPUÉS de suscribirse, con el seq global del kernel.
pub fn subscribe_envelopes() -> broadcast::Receiver<EventEnvelope> {
    EVENT_BUS.subscribe()
}

/// 017 — emisión SIN `AppHandle` (para callers runtime-agnósticos como el mobile
/// bridge, testeable sin Tauri). Aplica coalescing + asigna seq y publica el
/// envelope al fan-out (EVENT_BUS). Devuelve el envelope para que el caller lo
/// reenvíe también a las webview windows por su propio emisor (EmitFn) — así un
/// `ApprovalRequested` disparado desde el móvil llega a desktop Y a otros móviles
/// con el MISMO seq. Si se coalesció, devuelve `None`.
pub fn publish_envelope(ev: AppEvent) -> Option<EventEnvelope> {
    let env = prepare(ev)?;
    let _ = EVENT_BUS.send(env.clone());
    Some(env)
}

/// Helper de EMISIÓN — punto de entrada del contrato SSOT. Toda mutación de estado CRÍTICO
/// (tareas/sesiones/agentes/approvals/layout) debería terminar acá. Emite a TODAS las webview
/// windows por el canal único `BUS_CHANNEL` (Tauri v2 `emit` = broadcast a todas las ventanas).
/// Aplica coalescing antes de emitir. Best-effort: un fallo de IPC no debe tumbar la mutación.
pub fn emit_event(app: &tauri::AppHandle, ev: AppEvent) {
    use tauri::Emitter;
    if let Some(env) = prepare(ev) {
        if let Err(e) = app.emit(BUS_CHANNEL, &env) {
            tracing::debug!("event_bus emit failed (non-fatal): {}", e);
        }
        // 017 — fan-out al mobile bridge con el MISMO seq (best-effort).
        let _ = EVENT_BUS.send(env);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // NOTA: los tests de COALESCING/DEBOUNCE ejercitan un `Coalescer` LOCAL (no el global), para
    // ser deterministas. El global `COALESCER`/`SEQ` lo comparten todos los tests del binario que
    // corren en paralelo → un `reset` de un test pisaría el debounce de otro (carrera). El seq sí
    // se prueba contra el global porque es atómico y SÓLO afirmamos crecimiento estricto (robusto
    // a cualquier interleaving).

    #[test]
    fn seq_is_monotonic_across_emits() {
        // Eventos DISTINTOS (firmas distintas) nunca se coalescen → cada uno consume un seq, y
        // los seq son estrictamente crecientes (contrato: el front descarta seq menores). Usa el
        // path real `prepare` (global SEQ atómico): el crecimiento estricto se mantiene aunque
        // otros tests emitan en paralelo.
        let mut last = 0u64;
        for i in 0..50 {
            let env = prepare(AppEvent::TaskChanged {
                id: format!("seqtest-{i}"),
                state: "running".into(),
            })
            .expect("evento distinto no debe coalescer");
            assert!(
                env.seq > last,
                "seq debe ser monotónico: {} !> {}",
                env.seq,
                last
            );
            last = env.seq;
        }
    }

    #[test]
    fn semantic_events_never_coalesce() {
        // Audit codex+deepseek US3: los eventos SEMÁNTICOS (todos los actuales) NUNCA se coalescen —
        // dos ocurrencias idénticas consecutivas DENTRO de la ventana de debounce DEBEN emitir AMBAS
        // (cada ocurrencia importa: dos CommandExecuted reales, o un re-disparo de AgentStateChanged
        // con la misma firma, no se pierden).
        let mut c = Coalescer::new(DEFAULT_DEBOUNCE);
        let now = Instant::now();
        for ev in [
            AppEvent::AgentStateChanged {
                id: "ag-1".into(),
                state: "running".into(),
            },
            AppEvent::CommandExecuted {
                command_id: "council_run".into(),
            },
            AppEvent::ApprovalRequested {
                request_id: "r1".into(),
                command_id: "reset_furx".into(),
            },
        ] {
            assert!(c.should_emit(&ev, now), "1ra ocurrencia emite");
            assert!(
                c.should_emit(&ev, now),
                "2da ocurrencia IDÉNTICA también emite (no-coalescible)"
            );
            assert!(
                !ev.coalescible(),
                "los eventos semánticos no son coalescibles"
            );
        }
    }

    #[test]
    fn coalescer_mechanism_collapses_when_coalescible() {
        // El MECANISMO de coalescing (HashMap+debounce) sigue funcionando para futuras variantes
        // de alta frecuencia (progress/heartbeat) que devuelvan coalescible()==true. Hoy NINGÚN
        // evento lo es (YAGNI), así que probamos el mecanismo invocando el path interno con un
        // evento marcado como coalescible vía should_emit_for_coalescible (helper de test).
        let mut c = Coalescer::new(Duration::from_millis(50));
        let base = Instant::now();
        let sig = "progress:42".to_string();
        // path interno: mismo (kind, firma) dentro de la ventana → coalesce; fuera → re-emite.
        assert!(
            c.note_and_should_emit("Progress", &sig, base),
            "primero pasa"
        );
        assert!(
            !c.note_and_should_emit("Progress", &sig, base + Duration::from_millis(10)),
            "dentro coalesce"
        );
        assert!(
            c.note_and_should_emit("Progress", &sig, base + Duration::from_millis(60)),
            "post-debounce re-emite"
        );
    }

    #[test]
    fn stale_seq_is_ignored_contract() {
        // Simula "2 viewports": el contrato del lado front es descartar un envelope cuyo seq es
        // <= al último visto. Modelamos la regla acá para fijarla (el TS la implementa idéntica).
        // Eventos distintos → seq creciente (global SEQ atómico).
        let e1 = prepare(AppEvent::TaskChanged {
            id: "stale-1".into(),
            state: "running".into(),
        })
        .unwrap();
        let e2 = prepare(AppEvent::TaskChanged {
            id: "stale-1".into(),
            state: "done".into(),
        })
        .unwrap();
        assert!(e2.seq > e1.seq);

        // Un "viewport" ya vio e2 (el más nuevo). Llega e1 (viejo, p.ej. reordenado por el IPC).
        let mut last_seen = e2.seq;
        let apply = |incoming: u64, last_seen: &mut u64| -> bool {
            if incoming <= *last_seen {
                return false; // descartar: no pisar un snapshot nuevo con uno viejo
            }
            *last_seen = incoming;
            true
        };
        assert!(
            !apply(e1.seq, &mut last_seen),
            "seq viejo NO debe aplicarse"
        );
        // Un evento más nuevo (e3) sí se aplica y avanza el cursor.
        let e3 = prepare(AppEvent::TaskChanged {
            id: "stale-1".into(),
            state: "merged".into(),
        })
        .unwrap();
        assert!(apply(e3.seq, &mut last_seen), "seq nuevo debe aplicarse");
        assert_eq!(last_seen, e3.seq);
    }

    #[test]
    fn envelope_serializes_with_seq_ts_and_tag() {
        // El envelope serializa a JSON con seq + ts + tag/data planos (espejo del tipo TS).
        let env = prepare(AppEvent::CommandExecuted {
            command_id: "agent.run".into(),
        })
        .unwrap();
        let json = serde_json::to_value(&env).unwrap();
        assert!(json.get("seq").is_some());
        assert!(json.get("ts").is_some());
        assert_eq!(
            json.get("tag").and_then(|v| v.as_str()),
            Some("CommandExecuted")
        );
        assert_eq!(
            json.get("data")
                .and_then(|d| d.get("command_id"))
                .and_then(|v| v.as_str()),
            Some("agent.run")
        );
    }
}
