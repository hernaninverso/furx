// 021-voice-es — lógica PURA del indicador de dictado por voz.
//
// Dos responsabilidades, ambas testeables sin React/DOM:
//   1. "freeze destination": el pane destino del dictado se CONGELA al iniciar el PTT.
//      Aunque el foco cambie durante la grabación/transcripción, el texto va a ESE pane.
//   2. Mapeo de estados inequívocos: idle → recording → transcribing → idle.
//
// El front (Shell.tsx / VoiceModal.tsx) usa esto para anclar el indicador al destino
// correcto y mostrar el estado sin ambigüedad. El audio NUNCA sale de la Mac (whisper local).

/** Estado del indicador de voz. */
export type VoiceState = "idle" | "recording" | "transcribing";

/** Eventos que mueven la máquina de estados. */
export type VoiceEvent = "start" | "release" | "done" | "cancel";

/**
 * Un destino de dictado. `kind:"pane"` ancla al pane (con su id); `kind:"modal"`
 * ancla al VoiceModal; `kind:"none"` = sin pane visible (cae al pill global de respaldo).
 */
export type VoiceDestination =
  | { kind: "pane"; paneId: string }
  | { kind: "modal" }
  | { kind: "none" };

/**
 * Congela el destino al iniciar el dictado a partir del pane focuseado EN ESE INSTANTE.
 * Devuelve un destino inmutable: cambios de foco posteriores NO lo afectan (el caller
 * guarda este valor y lo usa para insertar el texto + anclar el indicador).
 */
export function freezeDestination(focusedPaneId: string | null): VoiceDestination {
  return focusedPaneId ? { kind: "pane", paneId: focusedPaneId } : { kind: "none" };
}

/**
 * ¿El destino congelado sigue siendo el mismo pane tras un cambio de foco?
 * Helper de claridad para tests/UX: el destino NO debe seguir al foco.
 */
export function destinationPaneId(dest: VoiceDestination): string | null {
  return dest.kind === "pane" ? dest.paneId : null;
}

/**
 * Transición de la máquina de estados. Pura y total (cualquier evento en cualquier
 * estado tiene salida definida). `cancel` siempre vuelve a idle.
 *
 * idle --start--> recording --release--> transcribing --done--> idle
 *                      \--cancel--> idle           \--cancel--> idle
 */
export function nextState(prev: VoiceState, event: VoiceEvent): VoiceState {
  if (event === "cancel") return "idle";
  switch (prev) {
    case "idle":
      return event === "start" ? "recording" : "idle";
    case "recording":
      // Al soltar la tecla pasamos a transcribir; un `done` directo (sin release) también cierra.
      if (event === "release") return "transcribing";
      if (event === "done") return "idle";
      return "recording";
    case "transcribing":
      return event === "done" ? "idle" : "transcribing";
    default:
      return "idle";
  }
}

/** Etiqueta breve, en español, para el estado actual (UI). */
export function stateLabel(state: VoiceState): string {
  switch (state) {
    case "recording":
      return "Grabando…";
    case "transcribing":
      return "Transcribiendo…";
    default:
      return "";
  }
}

/**
 * ¿Hay que mostrar el indicador anclado en ESTE pane?
 * Sólo cuando el dictado está activo (recording|transcribing) y el destino congelado
 * es exactamente este pane — NO el pane focuseado actual (de ahí el "freeze").
 */
export function anchorVisibleForPane(
  state: VoiceState,
  dest: VoiceDestination,
  paneId: string,
): boolean {
  if (state === "idle") return false;
  return dest.kind === "pane" && dest.paneId === paneId;
}

/**
 * F2 — decide CÓMO se muestra el indicador, dado el estado, el destino congelado y la
 * lista ACTUAL de panes visibles. Pura y total (testeable sin React).
 *
 *   - "anchor"     → hay dictado activo y el pane destino SIGUE existiendo/visible: el
 *                    indicador se ancla a ese pane (primario).
 *   - "globalPill" → hay dictado activo PERO el destino no es un pane visible: o porque
 *                    no había pane al iniciar (kind:"none"/"modal"), o porque el pane
 *                    destino se CERRÓ mientras grababa/transcribía (recordingPane stale).
 *                    Sin esto el indicador quedaría fantasma (ni ancla ni pill).
 *   - "none"       → idle (sin dictado).
 *
 * El antiguo criterio `!recordingPane` fallaba: si el pane destino se cerraba, el id
 * quedaba stale (truthy) → no había ancla (el pane no existe) NI pill global.
 */
export type IndicatorPlacement = "anchor" | "globalPill" | "none";

export function indicatorPlacement(
  state: VoiceState,
  dest: VoiceDestination,
  visiblePaneIds: readonly string[],
): IndicatorPlacement {
  if (state === "idle") return "none";
  if (dest.kind === "pane" && visiblePaneIds.includes(dest.paneId)) return "anchor";
  return "globalPill";
}

/**
 * ¿Mostrar el pill global de respaldo? Conveniencia sobre {@link indicatorPlacement}.
 * Útil cuando el caller sólo tiene el `paneId` del destino (string | null) y la lista
 * de panes visibles: el pill aparece cuando hay dictado activo y el destino NO está visible.
 */
export function globalPillVisible(
  state: VoiceState,
  dest: VoiceDestination,
  visiblePaneIds: readonly string[] = [],
): boolean {
  return indicatorPlacement(state, dest, visiblePaneIds) === "globalPill";
}
