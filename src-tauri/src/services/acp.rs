// services/acp.rs — 019 F4 (T040): cliente ACP (Agent Client Protocol) MÍNIMO y honesto.
//
// ACP = Agent Client Protocol (el protocolo de Zed para clientes de agentes): JSON-RPC 2.0 sobre
// stdio entre un CLIENTE (Furx) y un AGENTE (el binario que corre el LLM). El cliente abre el turno
// de prompt; el agente devuelve tool-calls, actualizaciones de sesión (streaming) y solicitudes de
// permiso. Ref: el shape real de ACP — métodos `initialize`, `session/new`, `session/prompt`, y las
// notificaciones `session/update` + la request server→client `session/request_permission`.
//
// ALCANCE de este cliente (honesto sobre lo cubierto):
//   - ✅ Núcleo de protocolo: framing JSON-RPC 2.0 (request/response/notification con id), el handshake
//     `initialize`, la creación de sesión `session/new`, el `session/prompt` y el STREAMING de updates
//     (`session/update`) hasta el `stop_reason` final. Las structs serializan/deserializan al shape ACP.
//   - ✅ `session/request_permission`: el agente pide permiso (server→client request); el cliente
//     responde con una opción (allow/reject). El núcleo del flujo de permisos está modelado y testeado.
//   - ✅ Transporte abstracto (`AcpTransport`): stdin/stdout del proceso agente vía un trait inyectable.
//     El transporte real (spawnear el binario y hablar por su stdio) lo materializa el caller; acá el
//     cliente es PURO (testeable con un transporte fake) — mismo patrón descriptivo que `agents.rs`.
//   - ⚠️ NO cubierto (documentado, no inventado): el catálogo COMPLETO de variantes de `session/update`
//     (plan/thought/tool_call_update con todos sus sub-tipos), `session/cancel`, `session/load`,
//     `fs/*` (read/write file) y `terminal/*` (ejecutar comandos) del lado cliente, y la negociación
//     fina de `capabilities`/`authMethods`. Se modela lo SUFICIENTE para un turno de prompt honesto;
//     lo demás se deja como `serde_json::Value` opaco (`extra`) para no fabricar un protocolo propio.
//
// BYOK (F-I): este cliente NUNCA proxea la API key. El agente ACP resuelve su credencial por su propia
// config/Keychain. El `env` que `agents.rs` arma para el spawn SÓLO transporta el comando del binario y
// banderas no-secretas (ver `acp_transport_env`); jamás un token.

use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::time::{Duration, Instant};

/// Versión de protocolo ACP que este cliente anuncia en `initialize`. ACP usa enteros monotónicos.
pub const ACP_PROTOCOL_VERSION: u32 = 1;

/// Deadline GLOBAL por default para un turno de prompt (`session/prompt`). El cliente lo impone él mismo
/// (no depende de que el transporte corte): si el turno excede este lapso → `Err("acp timeout")`. Esto
/// cubre TANTO un `recv` que bloquea (transporte mal portado) COMO un flood de mensajes del agente.
pub const DEFAULT_PROMPT_DEADLINE: Duration = Duration::from_secs(120);

/// Deadline GLOBAL por default para una request request/response síncrona (`initialize`, `session/new`).
/// Más corto que el del prompt: el handshake/creación de sesión no debería tardar.
pub const DEFAULT_CALL_DEADLINE: Duration = Duration::from_secs(60);

/// Tope DURO de iteraciones del loop de mensajes (defensa contra un agente que floodea
/// requests/notifications indefinidamente sin mandar nunca la response esperada). Complementa al
/// deadline temporal: aunque cada `recv` retorne instantáneamente, el loop no gira sin fin.
pub const MAX_LOOP_ITERATIONS: u64 = 100_000;

// ─────────────────────────────────────────────────────────────────────────────────────────────────
// Framing JSON-RPC 2.0
// ─────────────────────────────────────────────────────────────────────────────────────────────────

/// Un mensaje JSON-RPC entrante (del agente). ACP multiplexa 3 formas sobre el mismo stream:
///   - Response: `{jsonrpc, id, result|error}` — la respuesta a una request NUESTRA.
///   - Request:  `{jsonrpc, id, method, params}` — el agente nos pide algo (ej. request_permission).
///   - Notification: `{jsonrpc, method, params}` — sin id (ej. session/update streaming).
#[derive(Debug, Clone)]
pub enum Incoming {
    Response {
        id: u64,
        result: Option<Value>,
        error: Option<JsonRpcError>,
    },
    Request {
        id: u64,
        method: String,
        params: Value,
    },
    Notification {
        method: String,
        params: Value,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct JsonRpcError {
    pub code: i64,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

/// Serializa una request JSON-RPC 2.0 saliente (cliente→agente) a una línea NDJSON (sin el `\n`).
pub fn encode_request(id: u64, method: &str, params: Value) -> String {
    json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}).to_string()
}

/// Serializa una notification saliente (sin id).
pub fn encode_notification(method: &str, params: Value) -> String {
    json!({"jsonrpc": "2.0", "method": method, "params": params}).to_string()
}

/// Serializa una respuesta a una request del agente (cliente→agente), p.ej. responder a
/// `session/request_permission`.
pub fn encode_response_ok(id: u64, result: Value) -> String {
    json!({"jsonrpc": "2.0", "id": id, "result": result}).to_string()
}

/// Serializa una respuesta de error a una request del agente.
pub fn encode_response_err(id: u64, code: i64, message: &str) -> String {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}}).to_string()
}

/// Parsea una línea JSON-RPC entrante (del agente) a `Incoming`. Distingue response/request/notification
/// por la presencia de `id`/`method`/`result`/`error`, como manda JSON-RPC 2.0.
pub fn decode_incoming(line: &str) -> Result<Incoming> {
    let v: Value =
        serde_json::from_str(line.trim()).map_err(|e| anyhow!("json inválido: {}", e))?;
    let obj = v
        .as_object()
        .ok_or_else(|| anyhow!("mensaje no es objeto"))?;

    // JSON-RPC 2.0 estricto: el campo `jsonrpc` DEBE estar presente y ser exactamente "2.0".
    match obj.get("jsonrpc").and_then(|j| j.as_str()) {
        Some("2.0") => {}
        _ => return Err(anyhow!("jsonrpc inválido: se esperaba \"2.0\"")),
    }

    let has_id = obj.get("id").map(|i| !i.is_null()).unwrap_or(false);
    let has_method = obj.contains_key("method");

    match (has_id, has_method) {
        // id + method → request del agente hacia nosotros. `method` DEBE ser string.
        (true, true) => Ok(Incoming::Request {
            id: extract_id(obj)?,
            method: extract_method(obj)?,
            params: obj.get("params").cloned().unwrap_or(Value::Null),
        }),
        // id sin method → response a una request nuestra. Estricto: EXACTAMENTE uno de
        // {result, error} (XOR). Ni ninguno (¿qué respondió?) ni ambos (contradictorio) son válidos.
        (true, false) => {
            let has_result = obj.contains_key("result");
            let has_error = obj.get("error").map(|e| !e.is_null()).unwrap_or(false);
            if has_result == has_error {
                return Err(anyhow!(
                    "response JSON-RPC malformado: debe tener result XOR error"
                ));
            }
            Ok(Incoming::Response {
                id: extract_id(obj)?,
                result: obj.get("result").cloned(),
                error: obj
                    .get("error")
                    .filter(|e| !e.is_null())
                    .map(|e| serde_json::from_value(e.clone()))
                    .transpose()
                    .map_err(|e| anyhow!("error mal formado: {}", e))?,
            })
        }
        // method sin id → notification. `method` DEBE ser string.
        (false, true) => Ok(Incoming::Notification {
            method: extract_method(obj)?,
            params: obj.get("params").cloned().unwrap_or(Value::Null),
        }),
        (false, false) => Err(anyhow!("mensaje JSON-RPC sin id ni method")),
    }
}

fn extract_method(obj: &serde_json::Map<String, Value>) -> Result<String> {
    obj.get("method")
        .and_then(|m| m.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| anyhow!("method ausente o no es string"))
}

fn extract_id(obj: &serde_json::Map<String, Value>) -> Result<u64> {
    obj.get("id")
        .and_then(|i| i.as_u64())
        .ok_or_else(|| anyhow!("id ausente o no entero"))
}

// ─────────────────────────────────────────────────────────────────────────────────────────────────
// Structs del protocolo ACP (shape real)
// ─────────────────────────────────────────────────────────────────────────────────────────────────

/// Params de `initialize` (cliente→agente): anuncia versión + capacidades del cliente.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct InitializeParams {
    #[serde(rename = "protocolVersion")]
    pub protocol_version: u32,
    #[serde(rename = "clientCapabilities")]
    pub client_capabilities: ClientCapabilities,
}

/// Capacidades del cliente. Furx (local-first) declara que NO ofrece fs/terminal remotos por ahora
/// (cubierto: false honesto — no fabricamos soporte que no implementamos).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct ClientCapabilities {
    #[serde(default)]
    pub fs: FsCapabilities,
    #[serde(default)]
    pub terminal: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct FsCapabilities {
    #[serde(rename = "readTextFile", default)]
    pub read_text_file: bool,
    #[serde(rename = "writeTextFile", default)]
    pub write_text_file: bool,
}

/// Result de `initialize` (agente→cliente): versión negociada + capacidades del agente (opaco salvo
/// la versión, que es lo único que el núcleo necesita verificar).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct InitializeResult {
    #[serde(rename = "protocolVersion")]
    pub protocol_version: u32,
    #[serde(rename = "agentCapabilities", default)]
    pub agent_capabilities: Value,
    /// Métodos de auth disponibles (opaco). BYOK: el cliente NO los usa para inyectar keys.
    #[serde(rename = "authMethods", default)]
    pub auth_methods: Value,
}

/// Params de `session/new`: dónde corre (cwd = worktree del attempt) + servidores MCP (opaco).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct NewSessionParams {
    pub cwd: String,
    #[serde(rename = "mcpServers", default, skip_serializing_if = "Vec::is_empty")]
    pub mcp_servers: Vec<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct NewSessionResult {
    #[serde(rename = "sessionId")]
    pub session_id: String,
}

/// Params de `session/prompt`: la sesión + el prompt como bloques de contenido. Modelamos el bloque
/// de texto (lo único que el flujo best-of-N necesita: entregar el objetivo). Otros tipos de bloque
/// (image/resource/audio) NO están cubiertos — se dejarían como `ContentBlock::Other(Value)`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PromptParams {
    #[serde(rename = "sessionId")]
    pub session_id: String,
    pub prompt: Vec<ContentBlock>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentBlock {
    Text {
        text: String,
    },
    /// Cualquier otro tipo de bloque ACP que NO modelamos (image/resource/...). Honesto: opaco.
    #[serde(untagged)]
    Other(Value),
}

impl ContentBlock {
    pub fn text(s: impl Into<String>) -> Self {
        ContentBlock::Text { text: s.into() }
    }
}

/// Result de `session/prompt`: por qué terminó el turno.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PromptResult {
    #[serde(rename = "stopReason")]
    pub stop_reason: StopReason,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    EndTurn,
    MaxTokens,
    MaxTurnRequests,
    Refusal,
    Cancelled,
}

/// Params de la request `session/request_permission` (agente→cliente): el agente quiere ejecutar una
/// tool-call y nos pide elegir una de las `options`. Esto es lo que conecta ACP con el gate universal
/// de Furx (015): cada opción se mapea a allow/reject y la decisión se audita.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RequestPermissionParams {
    #[serde(rename = "sessionId")]
    pub session_id: String,
    #[serde(rename = "toolCall")]
    pub tool_call: ToolCall,
    pub options: Vec<PermissionOption>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolCall {
    #[serde(rename = "toolCallId")]
    pub tool_call_id: String,
    #[serde(default)]
    pub title: String,
    /// Tipo de la tool (edit/execute/read/...). Opaco salvo el rótulo.
    #[serde(default)]
    pub kind: String,
    /// El resto del tool_call (locations, raw_input, content) — opaco; lo consume la UI de review.
    #[serde(flatten)]
    pub extra: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PermissionOption {
    #[serde(rename = "optionId")]
    pub option_id: String,
    pub name: String,
    /// "allow_once" | "allow_always" | "reject_once" | "reject_always".
    pub kind: PermissionOptionKind,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PermissionOptionKind {
    AllowOnce,
    AllowAlways,
    RejectOnce,
    RejectAlways,
}

impl PermissionOptionKind {
    /// ¿Esta opción AUTORIZA la tool-call? (allow_* sí; reject_* no). El gate de Furx usa esto para
    /// mapear la decisión auditada → la opción ACP a devolver.
    pub fn is_allow(self) -> bool {
        matches!(
            self,
            PermissionOptionKind::AllowOnce | PermissionOptionKind::AllowAlways
        )
    }
}

/// La decisión que el cliente devuelve a `session/request_permission`. ACP espera
/// `{outcome: {outcome: "selected", optionId}}` o `{outcome: {outcome: "cancelled"}}`.
#[derive(Debug, Clone, PartialEq)]
pub enum PermissionDecision {
    Selected(String),
    Cancelled,
}

impl PermissionDecision {
    /// Serializa la decisión al `result` JSON-RPC que ACP espera.
    pub fn to_result(&self) -> Value {
        match self {
            PermissionDecision::Selected(id) => {
                json!({"outcome": {"outcome": "selected", "optionId": id}})
            }
            PermissionDecision::Cancelled => json!({"outcome": {"outcome": "cancelled"}}),
        }
    }
}

/// Elige, de las opciones que ofrece el agente, la que corresponde a la decisión del gate (allow/reject).
/// Prefiere `*_once` (single-use, como el gate de Furx). Si el gate dijo `allow=true` pero el agente NO
/// ofreció ninguna opción allow, devuelve `Cancelled` (fail-safe: no inventamos una autorización).
pub fn decide_permission(allow: bool, options: &[PermissionOption]) -> PermissionDecision {
    let want_allow = allow;
    // Buscar primero la variante `_once`, luego cualquier `_always` del mismo signo.
    let pick = options
        .iter()
        .find(|o| o.kind.is_allow() == want_allow && o.kind.is_once())
        .or_else(|| options.iter().find(|o| o.kind.is_allow() == want_allow));
    match pick {
        Some(o) => PermissionDecision::Selected(o.option_id.clone()),
        None => PermissionDecision::Cancelled,
    }
}

impl PermissionOptionKind {
    fn is_once(self) -> bool {
        matches!(
            self,
            PermissionOptionKind::AllowOnce | PermissionOptionKind::RejectOnce
        )
    }
}

// ─────────────────────────────────────────────────────────────────────────────────────────────────
// Transporte abstracto + cliente
// ─────────────────────────────────────────────────────────────────────────────────────────────────

/// Transporte de líneas NDJSON contra el agente ACP. La implementación REAL escribe a stdin del
/// proceso y lee de stdout (con timeout). El cliente es genérico sobre esto → testeable con un fake.
///
/// CONTRATO FUERTE de `recv_line` (la pieza load-bearing del fix de concurrencia HIGH):
/// el `deadline` (un `Instant` ABSOLUTO) NO es una sugerencia — es una OBLIGACIÓN del transporte.
/// Una implementación de `recv_line` DEBE retornar uno de:
///   - `Ok(Some(line))` — una línea entrante, o
///   - `Ok(None)`        — EOF (el agente cerró su stdout), o
///   - `Err(_)`          — error de I/O **o timeout** (deadline alcanzado sin línea disponible),
/// y DEBE hacerlo ANTES (o a más tardar AL alcanzar) el `deadline`. Un `read` que bloquea
/// indefinidamente más allá del `deadline` **VIOLA el contrato**.
///
/// Por qué vive acá y no en el cliente: el cliente es PURO (sync, sin async/threads); no puede
/// interrumpir un `read_line` bloqueante por sí solo. La GARANTÍA de no-cuelgue es del transporte.
/// La implementación real sobre el stdout del `Child` (hoy en BACKLOG — ver módulo doc) debe usar
/// un read CON timeout: `poll`/`select` sobre el fd, `SO_RCVTIMEO`, o un read no-bloqueante en loop
/// chequeando `Instant::now() >= deadline`. El cliente mantiene además su deadline GLOBAL como
/// backstop ENTRE iteraciones; pero el corte de un `recv` que de otro modo bloquearía depende de
/// que el transporte honre ESTE contrato.
pub trait AcpTransport {
    /// Envía una línea (sin `\n`; el transporte agrega el framing). Error si el pipe está roto.
    fn send_line(&mut self, line: &str) -> Result<()>;
    /// Recibe la próxima línea entrante respetando el `deadline` ABSOLUTO (ver contrato del trait).
    /// `Ok(None)` = EOF (el agente cerró). `Err` = I/O roto O deadline alcanzado sin línea. NUNCA
    /// debe bloquear más allá del `deadline` — eso colgaría al orquestador y viola el contrato.
    fn recv_line(&mut self, deadline: Instant) -> Result<Option<String>>;
}

/// Eventos de streaming que el cliente emite al caller durante un turno de prompt. El caller (best-of-N)
/// los reenvía al event_bus para "progreso vivo" SIN que el flujo conozca el protocolo ACP.
#[derive(Debug, Clone, PartialEq)]
pub enum TurnEvent {
    /// Una notificación `session/update` (streaming de texto/thought/tool_call). `params` opaco → la
    /// UI de review lo interpreta; el flujo sólo sabe "hubo progreso".
    Update(Value),
    /// El agente pidió permiso para una tool-call. El caller decide (vía gate) y el cliente responde.
    PermissionRequested(RequestPermissionParams),
}

/// Cliente ACP MÍNIMO. Mantiene el contador de ids JSON-RPC y la sesión activa. NO posee el proceso
/// (eso es del caller, igual que `agents.rs` no posee el PtyManager): habla por el `AcpTransport`.
pub struct AcpClient<T: AcpTransport> {
    transport: T,
    next_id: u64,
    session_id: Option<String>,
}

impl<T: AcpTransport> AcpClient<T> {
    pub fn new(transport: T) -> Self {
        Self {
            transport,
            next_id: 1,
            session_id: None,
        }
    }

    fn alloc_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    /// Handshake `initialize`. Envía la versión + capacidades del cliente y espera el result del agente.
    pub fn initialize(&mut self) -> Result<InitializeResult> {
        let params = InitializeParams {
            protocol_version: ACP_PROTOCOL_VERSION,
            client_capabilities: ClientCapabilities::default(),
        };
        let result = self.call("initialize", serde_json::to_value(&params)?)?;
        let parsed: InitializeResult = serde_json::from_value(result)?;
        Ok(parsed)
    }

    /// `session/new`: crea una sesión cuyo cwd es el worktree del attempt. Guarda el session_id.
    pub fn new_session(&mut self, cwd: &str) -> Result<String> {
        let params = NewSessionParams {
            cwd: cwd.to_string(),
            mcp_servers: vec![],
        };
        let result = self.call("session/new", serde_json::to_value(&params)?)?;
        let parsed: NewSessionResult = serde_json::from_value(result)?;
        self.session_id = Some(parsed.session_id.clone());
        Ok(parsed.session_id)
    }

    /// Espera la próxima línea NO vacía del transporte, imponiendo el deadline GLOBAL del cliente y el
    /// tope de iteraciones. Devuelve `Ok(Some(line))` con la línea, `Ok(None)` en EOF, o `Err` si el
    /// deadline expiró o se superó el tope de iteraciones (defensa contra recv que bloquea / flood de
    /// mensajes — el cliente corta SIN colgar al orquestador, independiente del transporte/agente).
    ///
    /// `deadline` es el `Instant` ABSOLUTO de corte del turno/call. Se pasa a CADA `recv_line` para que
    /// un read que de otro modo bloquearía se corte EN el recv (no sólo entre iteraciones): así un
    /// transporte stdio real que bloquea en `read_line` no cuelga al orquestador (ver contrato del trait).
    /// El check `Instant::now() >= deadline` ANTES del recv es el backstop del cliente entre vueltas.
    ///
    /// `iters` se incrementa por cada vuelta (cada línea drenada cuenta), de modo que el caller comparte
    /// un único contador a lo largo de TODO el turno (un flood de notifications/requests también gasta).
    fn recv_next_nonempty(
        &mut self,
        deadline: Instant,
        iters: &mut u64,
        ctx: &str,
    ) -> Result<Option<String>> {
        loop {
            if Instant::now() >= deadline {
                return Err(anyhow!("acp timeout durante {}", ctx));
            }
            *iters += 1;
            if *iters > MAX_LOOP_ITERATIONS {
                return Err(anyhow!(
                    "acp timeout (límite de iteraciones) durante {}",
                    ctx
                ));
            }
            // Pasamos el deadline ABSOLUTO al transporte: el recv mismo respeta el corte (contrato del
            // trait), de modo que un read bloqueante se interrumpe sin esperar a la próxima iteración.
            match self.transport.recv_line(deadline)? {
                Some(l) if !l.trim().is_empty() => return Ok(Some(l)),
                Some(_) => continue, // línea vacía (keep-alive): ignorar, pero ya gastó una iteración.
                None => return Ok(None),
            }
        }
    }

    /// `session/prompt`: abre el turno con el objetivo y procesa el STREAMING hasta el `stop_reason`.
    ///
    /// `on_event` recibe cada `TurnEvent` (update de streaming / pedido de permiso). Para los pedidos de
    /// permiso, el closure devuelve `Some(decision)` (allow/reject del gate) o `None` para cancelar.
    /// El cliente RESPONDE la request de permiso por el transporte — así el agente puede continuar.
    /// Las respuestas a OTRAS requests del agente (fs/terminal, no cubiertas) se rechazan con
    /// "method not found" (-32601) en vez de colgar el turno (honesto: no fabricamos soporte).
    pub fn prompt<F>(&mut self, objective: &str, on_event: F) -> Result<StopReason>
    where
        F: FnMut(TurnEvent) -> Option<PermissionDecision>,
    {
        self.prompt_with_deadline(objective, DEFAULT_PROMPT_DEADLINE, on_event)
    }

    /// Igual que `prompt` pero con un deadline GLOBAL configurable. El deadline lo impone el cliente
    /// (capturando `Instant::now()` al abrir el turno y chequeando `elapsed()` en cada vuelta del loop):
    /// cubre TANTO un `recv` que bloquea (transporte mal portado) COMO un flood de mensajes del agente
    /// (un agente que manda requests/notifications sin parar y nunca la response esperada). En ambos
    /// casos → `Err("acp timeout ...")` que CORTA el turno sin colgar al orquestador.
    pub fn prompt_with_deadline<F>(
        &mut self,
        objective: &str,
        deadline: Duration,
        mut on_event: F,
    ) -> Result<StopReason>
    where
        F: FnMut(TurnEvent) -> Option<PermissionDecision>,
    {
        let session_id = self
            .session_id
            .clone()
            .ok_or_else(|| anyhow!("no hay sesión activa (llamá new_session primero)"))?;
        let params = PromptParams {
            session_id,
            prompt: vec![ContentBlock::text(objective)],
        };
        let prompt_id = self.alloc_id();
        self.transport.send_line(&encode_request(
            prompt_id,
            "session/prompt",
            serde_json::to_value(&params)?,
        ))?;

        // Bucle de turno: drenamos updates/requests del agente hasta que llega la response a nuestro
        // `session/prompt` (con el stop_reason). Cualquier EOF antes de eso = fallo del turno.
        // Deadline GLOBAL + tope de iteraciones impuestos por el cliente (recv_next_nonempty), y el
        // deadline ABSOLUTO se propaga a cada recv (contrato del transporte) para cortar un recv bloqueante.
        let deadline = Instant::now() + deadline;
        let mut iters: u64 = 0;
        loop {
            let line = match self.recv_next_nonempty(deadline, &mut iters, "session/prompt")? {
                Some(l) => l,
                None => {
                    return Err(anyhow!(
                        "el agente cerró el stream antes de terminar el turno"
                    ))
                }
            };
            match decode_incoming(&line)? {
                Incoming::Response { id, result, error } if id == prompt_id => {
                    if let Some(err) = error {
                        return Err(anyhow!(
                            "session/prompt falló: {} ({})",
                            err.message,
                            err.code
                        ));
                    }
                    let res: PromptResult = serde_json::from_value(
                        result.ok_or_else(|| anyhow!("prompt sin result ni error"))?,
                    )?;
                    return Ok(res.stop_reason);
                }
                // Response a OTRA request nuestra: en este cliente mínimo no hay otras en vuelo durante
                // un prompt, así que la ignoramos defensivamente (no debería pasar).
                Incoming::Response { .. } => continue,
                Incoming::Notification { method, params } if method == "session/update" => {
                    on_event(TurnEvent::Update(params));
                }
                // Otras notificaciones (no cubiertas): ignorar sin romper.
                Incoming::Notification { .. } => continue,
                Incoming::Request { id, method, params }
                    if method == "session/request_permission" =>
                {
                    let req: RequestPermissionParams = serde_json::from_value(params)?;
                    let options = req.options.clone();
                    let decision = on_event(TurnEvent::PermissionRequested(req))
                        .unwrap_or(PermissionDecision::Cancelled);
                    // Validar que la opción elegida exista entre las ofrecidas (fail-safe).
                    let resolved = match &decision {
                        PermissionDecision::Selected(opt)
                            if options.iter().any(|o| &o.option_id == opt) =>
                        {
                            decision
                        }
                        PermissionDecision::Selected(_) => PermissionDecision::Cancelled,
                        PermissionDecision::Cancelled => PermissionDecision::Cancelled,
                    };
                    self.transport
                        .send_line(&encode_response_ok(id, resolved.to_result()))?;
                }
                // Request del agente que NO cubrimos (fs/read, terminal/execute, ...): responder
                // "method not found" en vez de colgar — honesto sobre lo no implementado.
                Incoming::Request { id, method, .. } => {
                    self.transport.send_line(&encode_response_err(
                        id,
                        -32601,
                        &format!("método no soportado por el cliente ACP de Furx: {}", method),
                    ))?;
                }
            }
        }
    }

    /// Helper request/response síncrono: envía y drena hasta encontrar la response con nuestro id.
    /// Notificaciones/requests intercaladas durante un initialize/new_session se ignoran/rechazan
    /// (no debería haber tool-calls antes del prompt). Usa el deadline `DEFAULT_CALL_DEADLINE`.
    fn call(&mut self, method: &str, params: Value) -> Result<Value> {
        self.call_with_deadline(method, params, DEFAULT_CALL_DEADLINE)
    }

    /// Igual que `call` con deadline GLOBAL explícito impuesto por el cliente: cubre tanto un `recv` que
    /// bloquea como un flood de mensajes intercalados → `Err("acp timeout ...")` sin colgar.
    fn call_with_deadline(
        &mut self,
        method: &str,
        params: Value,
        deadline: Duration,
    ) -> Result<Value> {
        let id = self.alloc_id();
        self.transport
            .send_line(&encode_request(id, method, params))?;
        // Deadline ABSOLUTO propagado a cada recv (contrato del transporte): corta un recv bloqueante.
        let deadline = Instant::now() + deadline;
        let mut iters: u64 = 0;
        loop {
            let line = match self.recv_next_nonempty(deadline, &mut iters, method)? {
                Some(l) => l,
                None => return Err(anyhow!("el agente cerró el stream durante {}", method)),
            };
            match decode_incoming(&line)? {
                Incoming::Response {
                    id: rid,
                    result,
                    error,
                } if rid == id => {
                    if let Some(err) = error {
                        return Err(anyhow!("{} falló: {} ({})", method, err.message, err.code));
                    }
                    return result.ok_or_else(|| anyhow!("{} sin result", method));
                }
                Incoming::Response { .. } => continue,
                Incoming::Notification { .. } => continue,
                // Un agente bien educado no manda requests antes del prompt; si lo hace, rechazar.
                Incoming::Request { id: rid, .. } => {
                    self.transport.send_line(&encode_response_err(
                        rid,
                        -32601,
                        "request inesperada antes del prompt",
                    ))?;
                }
            }
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────────────────────────
// Enganche con agents.rs (transporte vía SpawnPlan.env) — BYOK SIN proxear keys
// ─────────────────────────────────────────────────────────────────────────────────────────────────

/// Clave en `SpawnPlan.env` que marca que el attempt usa transporte ACP (no PTY clásico). El caller
/// (best-of-N / commands) lee esta clave para decidir si materializa un PTY o un `AcpClient`.
pub const ENV_TRANSPORT: &str = "FURX_AGENT_TRANSPORT";
/// Valor de `ENV_TRANSPORT` para ACP.
pub const TRANSPORT_ACP: &str = "acp";
/// Clave con el binario del agente ACP a spawnear (NO secreto). La key BYOK la resuelve el propio
/// agente por su config/Keychain — NUNCA viaja acá (F-I).
pub const ENV_ACP_BIN: &str = "FURX_ACP_BIN";

/// Construye el `env` (no-secreto) que `agents.rs` debe poner en el `SpawnPlan` para que el caller
/// sepa montar un transporte ACP. SÓLO transporta el binario + el flag de transporte; CERO credenciales.
pub fn acp_transport_env(agent_bin: &str) -> std::collections::HashMap<String, String> {
    let mut env = std::collections::HashMap::new();
    env.insert(ENV_TRANSPORT.to_string(), TRANSPORT_ACP.to_string());
    env.insert(ENV_ACP_BIN.to_string(), agent_bin.to_string());
    env
}

/// ¿Este `env` (de un `SpawnPlan`) pide transporte ACP? Helper para el caller.
pub fn is_acp_transport(env: &std::collections::HashMap<String, String>) -> bool {
    env.get(ENV_TRANSPORT).map(|v| v.as_str()) == Some(TRANSPORT_ACP)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    /// Transporte fake: cola scripteada de líneas entrantes + captura de salientes. Permite testear el
    /// cliente sin spawnear ningún proceso (cliente PURO).
    struct FakeTransport {
        incoming: VecDeque<Option<String>>,
        sent: Vec<String>,
        fail_recv: bool,
        /// Si `Some(line)`, el transporte devuelve SIEMPRE esa línea (flood): nunca se vacía ni manda la
        /// response esperada. Simula un agente que floodea requests/notifications sin fin → el cliente
        /// debe cortar por deadline/límite de iteraciones, NO loopear infinito ni bloquear.
        flood: Option<String>,
        /// Si `true`, el transporte simula un recv que BLOQUEARÍA (un stdio real sin datos): en vez de
        /// colgar, HONRA el contrato del trait y, al alcanzar el `deadline`, devuelve un timeout.
        /// Esto modela una implementación correcta de `recv_line` (poll/select con timeout sobre el fd).
        block_until_deadline: bool,
    }
    impl FakeTransport {
        fn new(incoming: Vec<String>) -> Self {
            Self {
                incoming: incoming.into_iter().map(Some).collect(),
                sent: vec![],
                fail_recv: false,
                flood: None,
                block_until_deadline: false,
            }
        }
        /// Transporte que floodea: cada `recv_line` devuelve `line` (típicamente una request entrante)
        /// indefinidamente, sin mandar nunca la response esperada.
        fn flooding(line: &str) -> Self {
            Self {
                incoming: VecDeque::new(),
                sent: vec![],
                fail_recv: false,
                flood: Some(line.to_string()),
                block_until_deadline: false,
            }
        }
        /// Transporte que NUNCA tiene datos (el agente está vivo pero mudo): un `recv_line` ingenuo
        /// bloquearía para siempre. Este fake, como manda el contrato, espera hasta el `deadline` y ahí
        /// devuelve `Err(timeout)` — exactamente lo que un transporte stdio real con timeout debe hacer.
        fn blocks_but_honors_deadline() -> Self {
            Self {
                incoming: VecDeque::new(),
                sent: vec![],
                fail_recv: false,
                flood: None,
                block_until_deadline: true,
            }
        }
    }
    impl AcpTransport for FakeTransport {
        fn send_line(&mut self, line: &str) -> Result<()> {
            self.sent.push(line.to_string());
            Ok(())
        }
        fn recv_line(&mut self, deadline: Instant) -> Result<Option<String>> {
            if self.fail_recv {
                return Err(anyhow!("timeout simulado"));
            }
            if self.block_until_deadline {
                // Un read bloqueante CORRECTO: en vez de colgar, esperamos hasta el deadline y cortamos.
                // (En producción esto sería poll/select con timeout sobre el fd; acá un busy-wait corto.)
                while Instant::now() < deadline {
                    std::thread::yield_now();
                }
                return Err(anyhow!("acp recv timeout: deadline alcanzado sin datos"));
            }
            if let Some(line) = &self.flood {
                return Ok(Some(line.clone()));
            }
            Ok(self.incoming.pop_front().flatten())
        }
    }

    #[test]
    fn encode_decode_roundtrip_request() {
        let line = encode_request(7, "session/prompt", json!({"x": 1}));
        match decode_incoming(&line).unwrap() {
            Incoming::Request { id, method, params } => {
                assert_eq!(id, 7);
                assert_eq!(method, "session/prompt");
                assert_eq!(params["x"], 1);
            }
            other => panic!("esperaba Request, fue {:?}", other),
        }
    }

    #[test]
    fn decode_distinguishes_message_kinds() {
        // response (id, sin method)
        let r = decode_incoming(r#"{"jsonrpc":"2.0","id":3,"result":{"ok":true}}"#).unwrap();
        assert!(matches!(r, Incoming::Response { id: 3, .. }));
        // notification (method, sin id)
        let n =
            decode_incoming(r#"{"jsonrpc":"2.0","method":"session/update","params":{}}"#).unwrap();
        assert!(matches!(n, Incoming::Notification { .. }));
        // request (id + method)
        let q = decode_incoming(
            r#"{"jsonrpc":"2.0","id":1,"method":"session/request_permission","params":{}}"#,
        )
        .unwrap();
        assert!(matches!(q, Incoming::Request { id: 1, .. }));
        // error response
        let e =
            decode_incoming(r#"{"jsonrpc":"2.0","id":9,"error":{"code":-32000,"message":"boom"}}"#)
                .unwrap();
        match e {
            Incoming::Response {
                error: Some(err), ..
            } => {
                assert_eq!(err.code, -32000);
                assert_eq!(err.message, "boom");
            }
            other => panic!("esperaba error response, fue {:?}", other),
        }
        // basura → Err, no panic
        assert!(decode_incoming("{not json").is_err());
        assert!(decode_incoming(r#"{"jsonrpc":"2.0"}"#).is_err());
    }

    #[test]
    fn decode_rejects_malformed_jsonrpc() {
        // jsonrpc != "2.0" (versión incorrecta) → Err.
        assert!(decode_incoming(r#"{"jsonrpc":"1.0","id":1,"result":{}}"#).is_err());
        // jsonrpc ausente → Err.
        assert!(decode_incoming(r#"{"id":1,"result":{}}"#).is_err());
        // jsonrpc no-string → Err.
        assert!(decode_incoming(r#"{"jsonrpc":2.0,"id":1,"result":{}}"#).is_err());

        // method no-string en una request (tiene id) → Err (no `"".unwrap_or_default()`).
        assert!(decode_incoming(r#"{"jsonrpc":"2.0","id":1,"method":123,"params":{}}"#).is_err());
        // method no-string en una notification (sin id) → Err.
        assert!(decode_incoming(r#"{"jsonrpc":"2.0","method":false,"params":{}}"#).is_err());

        // response con id pero SIN result ni error → malformado → Err.
        assert!(decode_incoming(r#"{"jsonrpc":"2.0","id":1}"#).is_err());
        // response con id y AMBOS result + error → malformado (contradictorio) → Err.
        assert!(decode_incoming(
            r#"{"jsonrpc":"2.0","id":1,"result":{"ok":true},"error":{"code":-1,"message":"x"}}"#
        )
        .is_err());

        // sanity: una response válida (result XOR error) SÍ decodifica.
        assert!(decode_incoming(r#"{"jsonrpc":"2.0","id":1,"result":{"ok":true}}"#).is_ok());
        assert!(
            decode_incoming(r#"{"jsonrpc":"2.0","id":1,"error":{"code":-1,"message":"x"}}"#)
                .is_ok()
        );
    }

    #[test]
    fn protocol_structs_serialize_to_acp_shape() {
        // initialize params usa los nombres camelCase del protocolo real.
        let p = InitializeParams {
            protocol_version: 1,
            client_capabilities: ClientCapabilities::default(),
        };
        let v = serde_json::to_value(&p).unwrap();
        assert_eq!(v["protocolVersion"], 1);
        assert!(v["clientCapabilities"]["fs"]["readTextFile"] == json!(false));

        // prompt block: {type:"text", text:"..."}
        let pp = PromptParams {
            session_id: "s1".into(),
            prompt: vec![ContentBlock::text("hacé X")],
        };
        let v = serde_json::to_value(&pp).unwrap();
        assert_eq!(v["sessionId"], "s1");
        assert_eq!(v["prompt"][0]["type"], "text");
        assert_eq!(v["prompt"][0]["text"], "hacé X");

        // stop_reason snake_case
        let pr: PromptResult = serde_json::from_value(json!({"stopReason": "end_turn"})).unwrap();
        assert_eq!(pr.stop_reason, StopReason::EndTurn);
    }

    #[test]
    fn full_turn_initialize_session_prompt() {
        // Script honesto de un turno: initialize → session/new → session/prompt con 1 update y stop.
        let incoming = vec![
            // response a initialize (id=1)
            r#"{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":1,"agentCapabilities":{},"authMethods":[]}}"#.into(),
            // response a session/new (id=2)
            r#"{"jsonrpc":"2.0","id":2,"result":{"sessionId":"sess-abc"}}"#.into(),
            // streaming update (notification, sin id)
            r#"{"jsonrpc":"2.0","method":"session/update","params":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"trabajando"}}}"#.into(),
            // response a session/prompt (id=3)
            r#"{"jsonrpc":"2.0","id":3,"result":{"stopReason":"end_turn"}}"#.into(),
        ];
        let mut client = AcpClient::new(FakeTransport::new(incoming));
        let init = client.initialize().unwrap();
        assert_eq!(init.protocol_version, 1);
        let sid = client.new_session("/wt/attempt-0").unwrap();
        assert_eq!(sid, "sess-abc");

        let mut updates = 0;
        let stop = client
            .prompt("hacé X", |ev| {
                if let TurnEvent::Update(_) = ev {
                    updates += 1;
                }
                None
            })
            .unwrap();
        assert_eq!(stop, StopReason::EndTurn);
        assert_eq!(updates, 1, "debió emitir 1 update de streaming");
    }

    #[test]
    fn permission_request_is_answered_via_gate() {
        // El agente pide permiso para una tool-call; el "gate" decide allow → respondemos allow_once.
        let incoming = vec![
            r#"{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":1}}"#.into(),
            r#"{"jsonrpc":"2.0","id":2,"result":{"sessionId":"s"}}"#.into(),
            // request de permiso (id=99) intercalada en el turno
            r#"{"jsonrpc":"2.0","id":99,"method":"session/request_permission","params":{"sessionId":"s","toolCall":{"toolCallId":"tc1","title":"editar archivo","kind":"edit"},"options":[{"optionId":"a","name":"Allow","kind":"allow_once"},{"optionId":"r","name":"Reject","kind":"reject_once"}]}}"#.into(),
            r#"{"jsonrpc":"2.0","id":3,"result":{"stopReason":"end_turn"}}"#.into(),
        ];
        let mut client = AcpClient::new(FakeTransport::new(incoming));
        client.initialize().unwrap();
        client.new_session("/wt").unwrap();
        let mut asked = false;
        client
            .prompt("editá", |ev| match ev {
                TurnEvent::PermissionRequested(req) => {
                    asked = true;
                    assert_eq!(req.tool_call.tool_call_id, "tc1");
                    Some(decide_permission(true, &req.options))
                }
                _ => None,
            })
            .unwrap();
        assert!(asked, "el cliente debió surfaceear el pedido de permiso");
        // La última línea enviada debe ser la respuesta a id=99 seleccionando la opción allow ("a").
        let sent = client.transport.sent.clone();
        let perm_reply = sent
            .iter()
            .find(|l| l.contains("\"id\":99"))
            .expect("debió responder la request de permiso");
        assert!(
            perm_reply.contains("\"optionId\":\"a\""),
            "got: {perm_reply}"
        );
        assert!(perm_reply.contains("\"selected\""));
    }

    #[test]
    fn decide_permission_prefers_once_and_failsafe() {
        let opts = vec![
            PermissionOption {
                option_id: "ao".into(),
                name: "Allow once".into(),
                kind: PermissionOptionKind::AllowOnce,
            },
            PermissionOption {
                option_id: "aa".into(),
                name: "Allow always".into(),
                kind: PermissionOptionKind::AllowAlways,
            },
            PermissionOption {
                option_id: "ro".into(),
                name: "Reject".into(),
                kind: PermissionOptionKind::RejectOnce,
            },
        ];
        // allow → prefiere la opción allow_once (single-use, como el gate de Furx).
        assert_eq!(
            decide_permission(true, &opts),
            PermissionDecision::Selected("ao".into())
        );
        // reject → la reject_once.
        assert_eq!(
            decide_permission(false, &opts),
            PermissionDecision::Selected("ro".into())
        );
        // fail-safe: el gate dijo allow pero el agente NO ofreció ninguna allow → cancelar (no inventar).
        let only_reject = vec![PermissionOption {
            option_id: "ro".into(),
            name: "Reject".into(),
            kind: PermissionOptionKind::RejectOnce,
        }];
        assert_eq!(
            decide_permission(true, &only_reject),
            PermissionDecision::Cancelled
        );
    }

    #[test]
    fn timeout_does_not_block_returns_err() {
        // El transporte simula timeout en recv → el cliente devuelve Err (el caller marca el attempt
        // failed), nunca cuelga.
        let mut t = FakeTransport::new(vec![]);
        t.fail_recv = true;
        let mut client = AcpClient::new(t);
        let r = client.initialize();
        assert!(r.is_err(), "timeout debe propagar Err, no bloquear");
    }

    #[test]
    fn prompt_with_request_flood_times_out_not_hangs() {
        // Un agente que floodea requests entrantes (fs/read) sin mandar NUNCA la response al prompt.
        // El cliente responde -32601 a cada una PERO debe cortar por el deadline GLOBAL: ni loop
        // infinito ni bloqueo del orquestador. Deadline corto para que el test sea rápido.
        let req =
            r#"{"jsonrpc":"2.0","id":50,"method":"fs/read_text_file","params":{"path":"/x"}}"#;
        let mut client = AcpClient::new(FakeTransport::flooding(req));
        // Sembramos la sesión sin pasar por initialize/new_session (el flood nunca devolvería su response).
        client.session_id = Some("s".into());
        let start = std::time::Instant::now();
        let r = client.prompt_with_deadline("hacé X", Duration::from_millis(50), |_| None);
        assert!(
            r.is_err(),
            "el flood de requests debe terminar en Err (timeout), no en loop infinito"
        );
        assert!(
            r.unwrap_err().to_string().contains("timeout"),
            "el error debe ser un timeout del cliente"
        );
        // No debe haber tardado eternidades (cota generosa: el deadline es 50ms).
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "el cliente no debe bloquear al orquestador"
        );
        // Y debió ir respondiendo -32601 a las requests floodeadas (no las dejó colgadas).
        assert!(
            client.transport.sent.iter().any(|l| l.contains("-32601")),
            "debió rechazar las requests no soportadas con -32601"
        );
    }

    #[test]
    fn recv_blocking_is_cut_by_deadline_in_recv_not_between_iterations() {
        // CASO DEL FIX HIGH: un transporte cuyo `recv_line` bloquearía para siempre (agente vivo pero
        // mudo, NUNCA manda una línea). Con el deadline-en-recv, el transporte honra el contrato y corta
        // EN el propio recv al alcanzar el deadline → el cliente devuelve Err sin colgar al orquestador.
        // (Antes del fix, el deadline sólo se chequeaba ENTRE iteraciones; como nunca volvía del recv,
        //  la siguiente iteración jamás llegaba y el orquestador quedaba colgado.)
        let mut client = AcpClient::new(FakeTransport::blocks_but_honors_deadline());
        client.session_id = Some("s".into());
        let start = std::time::Instant::now();
        let r = client.prompt_with_deadline("hacé X", Duration::from_millis(50), |_| None);
        assert!(
            r.is_err(),
            "un recv bloqueante que respeta el deadline debe terminar en Err, no colgar"
        );
        // El corte vino del recv del transporte (contrato), propagado tal cual por el cliente.
        assert!(
            r.unwrap_err().to_string().contains("timeout"),
            "el error debe ser un timeout (del recv que honró el deadline)"
        );
        // Y debió respetar el deadline (no esperar eternidades): margen generoso sobre los 50ms.
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "el recv debió cortar cerca del deadline, sin bloquear al orquestador"
        );
    }

    #[test]
    fn call_with_flood_times_out_not_hangs() {
        // Mismo escenario sobre el camino request/response síncrono (initialize): un agente que floodea
        // notifications sin mandar nunca la response a initialize → el cliente corta por deadline.
        let note = r#"{"jsonrpc":"2.0","method":"session/update","params":{}}"#;
        let mut client = AcpClient::new(FakeTransport::flooding(note));
        let start = std::time::Instant::now();
        let r = client.call_with_deadline("initialize", json!({}), Duration::from_millis(50));
        assert!(
            r.is_err(),
            "el flood debe terminar en Err (timeout), no colgar"
        );
        assert!(r.unwrap_err().to_string().contains("timeout"));
        assert!(start.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn eof_before_turn_end_is_error() {
        // initialize ok, pero el agente cierra el stream antes de responder el prompt → Err honesto.
        let incoming = vec![
            r#"{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":1}}"#.into(),
            r#"{"jsonrpc":"2.0","id":2,"result":{"sessionId":"s"}}"#.into(),
            // (sin response a session/prompt → EOF)
        ];
        let mut client = AcpClient::new(FakeTransport::new(incoming));
        client.initialize().unwrap();
        client.new_session("/wt").unwrap();
        let r = client.prompt("x", |_| None);
        assert!(r.is_err());
    }

    #[test]
    fn unsupported_agent_request_is_rejected_not_hung() {
        // El agente pide fs/read (no cubierto) → respondemos -32601 y seguimos hasta el stop.
        let incoming = vec![
            r#"{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":1}}"#.into(),
            r#"{"jsonrpc":"2.0","id":2,"result":{"sessionId":"s"}}"#.into(),
            r#"{"jsonrpc":"2.0","id":50,"method":"fs/read_text_file","params":{"path":"/x"}}"#
                .into(),
            r#"{"jsonrpc":"2.0","id":3,"result":{"stopReason":"end_turn"}}"#.into(),
        ];
        let mut client = AcpClient::new(FakeTransport::new(incoming));
        client.initialize().unwrap();
        client.new_session("/wt").unwrap();
        let stop = client.prompt("x", |_| None).unwrap();
        assert_eq!(stop, StopReason::EndTurn);
        let rejected = client
            .transport
            .sent
            .iter()
            .any(|l| l.contains("\"id\":50") && l.contains("-32601"));
        assert!(
            rejected,
            "la request no soportada debió rechazarse con -32601"
        );
    }

    #[test]
    fn acp_transport_env_has_no_secrets() {
        // BYOK (F-I): el env del transporte ACP transporta SÓLO el binario + el flag; cero credenciales.
        let env = acp_transport_env("claude-code-acp");
        assert_eq!(
            env.get(ENV_TRANSPORT).map(String::as_str),
            Some(TRANSPORT_ACP)
        );
        assert_eq!(
            env.get(ENV_ACP_BIN).map(String::as_str),
            Some("claude-code-acp")
        );
        assert!(is_acp_transport(&env));
        // Ninguna clave/valor parece un secreto.
        for (k, v) in &env {
            let kv = format!("{k}={v}").to_lowercase();
            assert!(
                !["key", "token", "secret", "password", "bearer", "sk-"]
                    .iter()
                    .any(|m| kv.contains(m)),
                "env del transporte ACP no debe contener secretos: {kv}"
            );
        }
    }
}
