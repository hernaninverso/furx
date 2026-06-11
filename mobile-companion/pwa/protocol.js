// protocol.js — 017 T003/T044 · contrato compartido PWA-side de los frames del
// bridge móvil (reforma bottom-nav). ESPEJO de src-tauri/src/services/mobile_bridge.rs.
//
// El móvil reusa los primitivos SSOT del kernel transportados por el WS bridge:
//   - NavSpec        (server→client) — subset curado de dominios derivado de navGroups.
//   - CommandCatalog (server→client) — proyección del command_registry filtrada por visibility.
//   - AppEvent       (server→client) — { tag, data, seq, ts } del event bus del kernel.
//   - ExecuteCommand (client→server, FIRMADO) — solicitud de ejecución por command-id ref.
//
// Defense-in-depth (council 017 #4): los 3 frames server→client van FIRMADOS con
// un tag HMAC propio + envelope { sig, nonce, ts }. El móvil DEBE verificar la firma
// antes de aplicarlos (un exit node Tailscale comprometido podría MITM).

// Versión del protocolo móvil (handshake Hello). Mismatch desktop/PWA → degradar
// a la vista de sesión actual sin romper (FR-016).
export const MOBILE_PROTOCOL_VERSION = 1;

// Versión del shape del NavSpec (debe coincidir con MOBILE_NAV_SPEC_VERSION en TS).
export const NAV_SPEC_VERSION = 1;

// Tags HMAC de 8 bytes para los frames server→client firmados (017). Deben coincidir
// byte-a-byte con los tags del Rust (verify_signed_outbound). Los client→server (incl.
// ExecCmd_) viven en furx-sign.js.
export const SERVER_FRAME_TAGS = {
  nav_spec: "NavSpec_",
  command_catalog: "CmdCatlg",
  app_event: "AppEvnt_",
};

// Tipos de AppEvent del kernel (espejo de event_bus.rs::AppEvent / eventBus.ts).
export const APP_EVENT_TAGS = [
  "TaskChanged",
  "AgentStateChanged",
  "LayoutChanged",
  "CommandExecuted",
  "ApprovalRequested",
];

// Risks que SIEMPRE quedan pending-approval al ejecutarse desde móvil (FR-007).
// SÓLO informativo para la UI; la authz REAL la enforcea el backend (T061).
export const APPROVAL_RISKS = ["destructive", "credential"];
