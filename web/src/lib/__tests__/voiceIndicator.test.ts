// 021-voice-es — tests de la lógica PURA del indicador de voz (freeze destination + estados).
import {
  freezeDestination,
  destinationPaneId,
  nextState,
  stateLabel,
  anchorVisibleForPane,
  globalPillVisible,
  indicatorPlacement,
  type VoiceDestination,
} from "../voiceIndicator.ts";

let pass = 0, fail = 0;
function ok(cond: boolean, name: string) { if (cond) pass++; else { fail++; console.log(`FAIL ${name}`); } }

// ── freeze destination ───────────────────────────────────────────────────────
// El destino se congela al iniciar; un cambio de foco posterior NO lo mueve.
const dest = freezeDestination("pane-A");
ok(destinationPaneId(dest) === "pane-A", "freeze captura el pane focuseado al iniciar");
// Simular cambio de foco: el destino congelado NO cambia (es un valor inmutable).
const focusedNow = "pane-B";
ok(destinationPaneId(dest) === "pane-A", "el destino NO sigue al foco (pane-A se mantiene)");
ok(focusedNow !== destinationPaneId(dest), "foco actual (B) != destino congelado (A)");
// Sin pane focuseado → destino "none" (cae al pill global).
ok(freezeDestination(null).kind === "none", "sin foco → destino none");

// ── máquina de estados ───────────────────────────────────────────────────────
ok(nextState("idle", "start") === "recording", "idle+start → recording");
ok(nextState("recording", "release") === "transcribing", "recording+release → transcribing");
ok(nextState("transcribing", "done") === "idle", "transcribing+done → idle");
// Flujo feliz completo.
let s = nextState("idle", "start");
s = nextState(s, "release");
s = nextState(s, "done");
ok(s === "idle", "ciclo completo idle→recording→transcribing→idle");
// Cancel siempre vuelve a idle.
ok(nextState("recording", "cancel") === "idle", "recording+cancel → idle");
ok(nextState("transcribing", "cancel") === "idle", "transcribing+cancel → idle");
// Total: eventos inesperados no rompen.
ok(nextState("idle", "done") === "idle", "idle+done → idle (no-op)");
ok(nextState("recording", "start") === "recording", "recording+start → recording (idempotente)");

// ── labels ───────────────────────────────────────────────────────────────────
ok(stateLabel("recording") === "Grabando…", "label recording en español");
ok(stateLabel("transcribing") === "Transcribiendo…", "label transcribing en español");
ok(stateLabel("idle") === "", "label idle vacío");

// ── visibilidad del ancla por pane ───────────────────────────────────────────
const dPane: VoiceDestination = { kind: "pane", paneId: "pane-A" };
ok(anchorVisibleForPane("recording", dPane, "pane-A"), "ancla visible en el pane destino");
ok(!anchorVisibleForPane("recording", dPane, "pane-B"), "ancla NO visible en otro pane");
ok(!anchorVisibleForPane("idle", dPane, "pane-A"), "ancla NO visible en idle");
// Transcribiendo también muestra el ancla en el destino.
ok(anchorVisibleForPane("transcribing", dPane, "pane-A"), "ancla visible transcribiendo en destino");

// ── pill global de respaldo ──────────────────────────────────────────────────
const dNone: VoiceDestination = { kind: "none" };
const dModal: VoiceDestination = { kind: "modal" };
// Con el pane destino AÚN visible, NO hay pill (el ancla es el primario).
ok(!globalPillVisible("recording", dPane, ["pane-A"]), "pill global NO visible cuando el pane destino está visible");
// Destino sin pane → pill global de respaldo.
ok(globalPillVisible("recording", dNone, []), "pill global visible cuando destino=none");
ok(globalPillVisible("recording", dModal, []), "pill global visible cuando destino=modal (sin ancla de pane)");
ok(!globalPillVisible("idle", dNone, []), "pill global NO visible en idle");

// ── F2 · indicatorPlacement (anchor | globalPill | none) ─────────────────────
// idle → none.
ok(indicatorPlacement("idle", dPane, ["pane-A"]) === "none", "idle → none");
// Pane destino visible → anchor.
ok(indicatorPlacement("recording", dPane, ["pane-A"]) === "anchor", "destino visible → anchor (recording)");
ok(indicatorPlacement("transcribing", dPane, ["pane-A", "pane-B"]) === "anchor", "destino visible entre varios → anchor (transcribing)");
// F2 clave: el pane destino se CERRÓ mientras grababa/transcribía → recordingPane stale.
// No debe quedar fantasma: cae al pill global.
ok(indicatorPlacement("recording", dPane, ["pane-B"]) === "globalPill", "pane destino cerrado (recording) → globalPill, no fantasma");
ok(indicatorPlacement("transcribing", dPane, []) === "globalPill", "pane destino cerrado (transcribing, sin panes) → globalPill");
// Sin pane destino desde el inicio → pill global.
ok(indicatorPlacement("recording", dNone, ["pane-A"]) === "globalPill", "destino none → globalPill aunque haya panes");
ok(indicatorPlacement("recording", dModal, ["pane-A"]) === "globalPill", "destino modal → globalPill");

console.log(`voiceIndicator: ${pass} pass, ${fail} fail`);
if (fail > 0) process.exit(1);
