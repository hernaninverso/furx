import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "./lib/invoke"; // 015 T015: invoke con flujo de aprobación universal
import { listen } from "@tauri-apps/api/event";
import { Terminal } from "./Terminal";
import DataViewer from "./components/DataViewer";
import { AttentionBadge } from "./components/AttentionBadge";
import { PaneCardStrip } from "./components/PaneCardStrip";
import ComparatorView from "./components/ComparatorView";
import WebPane from "./components/WebPane";
import ContextPane from "./components/ContextPane";
import { SettingsView } from "./Settings";
import { Wizard } from "./Wizard";
import { CommandPalette, PaletteMode } from "./CommandPalette";
import { CommandPalette015, type ContextAction } from "./components/CommandPalette015";
// 016 Fase 1.5 — Discoverability/Onboarding (cada uno detrás de su flag).
import { HelpCenter } from "./components/HelpCenter";
import { WhatsNew } from "./components/WhatsNew";
import { Tour } from "./components/Tour";
import { isFirstRunDone, markFirstRunDone } from "./lib/tour";
import { stateLabel, indicatorPlacement, freezeDestination, type VoiceState } from "./lib/voiceIndicator";
import { useT } from "./lib/i18n";
import { trackEvent, refreshTelemetryConfig } from "./lib/telemetry";
import { buildActions } from "./actions";
import { RestoreModal, FurxSession } from "./components/RestoreModal";
import { CouncilModal } from "./components/CouncilModal";
import { ConnectScreen } from "./wizard/ConnectScreen";
import type { LicenseState } from "./types";
import { SuggestionConfirm, SuggestionAction } from "./components/SuggestionConfirm";
import { VoiceModal as VoiceModalReal } from "./components/VoiceModal";
import { TopBar } from "./components/TopBar";
import { AuditDrawer } from "./components/AuditDrawer";
import { BroadcastModal } from "./components/BroadcastModal";
import { SmartPasteModal } from "./components/SmartPasteModal";
import { SaasView } from "./views/SaasView";
import { McpHealthView } from "./views/McpHealthView";
import { HeatmapView } from "./views/HeatmapView";
import { GrafanaView } from "./views/GrafanaView";
import { SshView } from "./views/SshView";
import { VpnView } from "./views/VpnView";
import { LatencyView } from "./views/LatencyView";
import { ReliabilityView } from "./views/ReliabilityView";
import { SavingsMeter } from "./components/SavingsMeter";
import { ActivityView } from "./views/ActivityView"; // 057 — Action Center
import { SearchView } from "./views/SearchView";
import { EvalView } from "./views/EvalView";
import { QueueView } from "./views/QueueView";
import { RouterView } from "./views/RouterView";
import { ReplayView } from "./views/ReplayView";
import { B4View } from "./views/B4View";
import { ExtensionsView } from "./views/ExtensionsView";
// 015 T030 — vistas de los huérfanos rescatados.
import { CrashLogView } from "./views/CrashLogView";
import { GithubView } from "./views/GithubView";
import { MemoryView } from "./views/MemoryView";
// 053 — nuevas vistas para comandos que tenían backend sin UI.
import { PolicyView } from "./views/PolicyView";
import { PresetView } from "./views/PresetView";
import { CardsRail } from "./components/CardsRail";
import { EmptyShellState } from "./components/EmptyShellState";
import { ShortcutSheet } from "./components/ShortcutSheet";
import { UpdateBanner } from "./components/UpdateBanner";
import { InfraBanner } from "./components/InfraBanner"; // 042 FR-003 — "inferencia no configurada".
import { SidebarGroups, SidebarGroupSpec } from "./components/SidebarGroups";
import { StandupModal } from "./components/StandupModal";
import { PrDescriptionModal } from "./components/PrDescriptionModal";
import { DisagreementModal } from "./components/DisagreementModal";
import { ToastStack } from "./components/ToastStack";
import { AgentGallery } from "./components/AgentGallery";
import { OrchestrationBoard } from "./components/OrchestrationBoard";
import { InterPaneSendModal } from "./components/InterPaneSendModal";
import { GlobalApprovalModal } from "./components/GlobalApprovalModal";
import { ErrorBoundary } from "./components/ErrorBoundary";
import { MergeReviewModal } from "./components/MergeReviewModal";
// 018 Fase 2 US1 — Workspace desde LayoutConfigV1 (dockview) detrás del flag newWorkspace.
import { WorkspaceView } from "./components/WorkspaceView";
// 018 Fase 2 US2 — botón detach/re-attach por pane (sólo cuando el pane vive en el WorkspaceView).
import { DetachButton } from "./components/DetachButton";
// BLOQUE A — extracted hooks (PLAN_CLOSE J): keep Shell.tsx as router+state,
// move polling / buffer details to dedicated files.
import { usePolling } from "./hooks/usePolling";
import { usePaneBuffers } from "./hooks/usePaneBuffers";
import { useToast } from "./hooks/useToast";
import type { Card, AuditEvent, MonitorSnapshot, MonitorResult, UsageSummary, AieState, Suggestion, PaneCfg, PaneMode, ClaudeAccount, AgentProfile, OrchTask } from "./types";
import { fmtWhen, fmtTime } from "./types";
// 015 T013 (US9) — el union `View` y el router interno viven en lib/router (SSOT compartido con
// el palette y la futura nav agrupada T020).
import { View, createNavigator } from "./lib/router";
// 015 T020 — feature-flag de la nav agrupada (su mecanismo de rollback a lista plana).
import { useFlag } from "./lib/flags";
import { parsePttHotkey, matchesPttHotkey, pttModifierKeyNames, formatPttHotkey, DEFAULT_PTT_HOTKEY, isPttCapturing } from "./lib/pttHotkey";
import { useAppEvent } from "./lib/eventBus";
import { NAV_GROUPS, buildNavSpec, navGroupLabelKey, navItemLabelKey } from "./lib/navGroups";
import { Smartphone } from "lucide-react"; // 055 wedge — CTA móvil en la barra de marca
// 022 US2 — derivación pura del label de pane (perfil ?? cuenta ?? slug), sin "A/B".
import { derivePaneLabel } from "./lib/paneLabel";
// 022 US8 — mode EFECTIVO del pane: si referencia un perfil, el mode se DERIVA del perfil
// (SSOT con el spawn de Rust) para que la captura de scrollback / label de sesión apunten
// a la MISMA sesión tmux que el backend crea, sin divergencia.
// 022 P0b · REFORMA 3 — stats accionables (drill-down) + REFORMA 4 — shortcuts del registry.
import { buildSidebarStats, statFilterToViewFilter, freshnessLabel, nextNavState, type ViewFilter, type ActionableStat } from "./lib/stats";
import { featuredSidebarShortcuts } from "./lib/sidebarShortcuts";
// 022 P1 · US6 — lógica pura del inbox de incidentes (agrupación, filtro, snooze, ir-al-origen).
import {
  inboxCards, inboxState, isSnoozed, groupIncidents, sourceTarget,
  computeSnoozeUntil, SNOOZE_OPTIONS, type GroupBy, type SnoozeOption,
  // 044 FR-002 — colapso/persistencia de grupos + badge de emergencia + cap de DOM.
  groupHasCritical, initialCollapsedState, loadCollapsedState, saveCollapsedState,
  INCIDENT_GROUP_INITIAL_VISIBLE, INCIDENT_GROUP_VISIBLE_STEP, INCIDENT_GROUP_DOM_CAP,
  // 050 FR-004 — modo compacto de incidentes (densidad alta, persistido; default OFF).
  loadCompactIncidents, saveCompactIncidents,
} from "./lib/incidents";
// 042 FR-001/FR-005 — lógica pura del gate del wizard en el boot + fallsafe local (testeado sin shell).
import { decideBoot, decideBootTimeout, firstRunCompletedLocal } from "./lib/boot";
// 044 FR-003 — guard anti doble-respuesta para las acciones de card (seq + timeout 15s + error inline).
import { useDecideGuard } from "./lib/decideGuardHook";
import { makeIdempotencyKey } from "./lib/idempotency"; // 050 FR-004 — key idempotente por decisión (multi-instancia futuro).

// spec-022 US2 — MODE_META solo conserva ícono/color de los modos legacy SIN slug.
// Los labels de cuenta ("Claude · A/B") quedaron ELIMINADOS: el label de cada pane
// se deriva en cadena `perfil ?? cuenta.label ?? slug` vía `derivePaneLabel`
// (lib/paneLabel.ts). Nunca "A/B" hardcodeado.
const MODE_META: Partial<Record<PaneMode, { icon: string; color: string }>> = {
  "zsh": { icon: ">_", color: "#6c7b91" },
  "codex": { icon: "◇", color: "var(--amber)" },
  "gemini": { icon: "✦", color: "var(--green)" },
  "aider": { icon: "⌘", color: "var(--red)" },
};

interface LayoutFromDB {
  id: string; name: string; panes: PaneCfg[]; grid_cols: string; grid_rows: string;
}

// 059 — el helper `isDevBuild` y el botón Seed-demo-cards (dev) del sidebar se removieron (ensuciaban
// la superficie de producto, visibles en el build dev). El comando `seed_demo_cards` sigue en el
// registry como dev-only (extra.dev_only=true): la palette universal lo EXCLUYE en producción y el
// backend lo rechaza en release → en el producto que se shipea NO hay forma de dispararlo.

export function Shell({ version }: { version: string }) {
  const t = useT(); // 016 — copy del onboarding (tour offer) vía i18n.
  const [view, setView] = useState<View>("panes");
  // 022 P0b · REFORMA 3 — filtro inicial que un stat accionable aplica a su vista destino
  // (incidentes→abiertos, monitors→down). Efímero / window-scoped. `null` = sin filtro.
  const [viewFilter, setViewFilter] = useState<ViewFilter>(null);
  // 022 P0b (audit 3-frontera MED) — el filtro de drill-down es ONE-SHOT: lo setea SÓLO
  // `goToStat` (entrar a una vista por un stat). Toda nav NORMAL (sidebar, palette `view.*`,
  // deeplinks, atajos) pasa por `navigate(view)` que SIEMPRE reescribe el filtro (default `null`)
  // → re-entrar a Incidents/Monitors por otro camino lo ve limpio. Setter atómico (view+filter):
  // evita el orden frágil de un efecto que limpie el filtro al cambiar de view.
  const navigate = useCallback((v: View, filter: ViewFilter = null) => {
    const next = nextNavState(v, filter); // reducer puro (one-shot) — invariante testeado en stats.test
    setViewFilter(next.filter);
    setView(next.view);
  }, []);
  // 022 P0b — timestamp del último refresh de cards/monitors (freshness de los stats).
  const [lastRefreshAt, setLastRefreshAt] = useState<number | null>(null);
  // 015 T020 — nav agrupada (true) vs lista plana (false = rollback). Persiste en localStorage.
  // 059 — el toggle visible "Agrupado/Plano" se removió del sidebar (afordance de rollback que el
  // usuario final no necesita); la nav agrupada (055) ES el producto. El flag `groupedNav` (default true)
  // sigue controlando el rollback a lista plana sin exponer un botón.
  const [navGrouped] = useFlag("groupedNav");
  // 018 Fase 2 US1 — Workspace flexible (dockview desde LayoutConfigV1) vs grilla 2×2 legacy.
  // ON  = render por árbol Split/Tabs/Leaf (SSOT versionado). OFF = grilla legacy (ROLLBACK).
  // TODO(018): retirar el camino legacy y este flag una vez validada la persistencia v1 en
  //            producción — FECHA DE RETIRO OBJETIVO: 2026-07-15 (council: no camino dual perpetuo).
  const [newWorkspace] = useFlag("newWorkspace");
  const [panes, setPanes] = useState<PaneCfg[]>([]);
  const [gridCols, setGridCols] = useState<string>("1fr 1fr");
  const [gridRows, setGridRows] = useState<string>("1fr 1fr");
  const [focusedPane, setFocusedPane] = useState<string | null>(null);
  const [cards, setCards] = useState<Card[]>([]);
  // 044 FR-002 — estado de carga del inbox de cards (skeleton/error/retry en CardsView).
  const [cardsLoading, setCardsLoading] = useState<boolean>(true);
  const [cardsError, setCardsError] = useState<string | null>(null);
  const [monitors, setMonitors] = useState<MonitorSnapshot[]>([]);
  const [events, setEvents] = useState<AuditEvent[]>([]);
  const [needsWizard, setNeedsWizard] = useState(false);
  // 042 FR-001 — estado del primer fetch de settings del boot. `loading` hasta que `settings_get`
  // (first-run) + `tmux_available` resuelvan (vía allSettled, un fallo no tumba el boot) o hasta el
  // timeout DURO de 8s. Mientras `loading` mostramos un spinner SIN flash (delay ~150ms). En `error`
  // (settings no cargó / timeout) caemos al wizard para no dejar al usuario en una shell sin contexto.
  const [settingsState, setSettingsState] = useState<"loading" | "loaded" | "error">("loading");
  const [connectOpen, setConnectOpen] = useState(false);
  // HIGH-1 fix (Codex): start in "pending" (null) — NOT true.
  // While we don't know the license state, ⌘J should NOT auto-grant Pro.
  const [proActive, setProActive] = useState<boolean | null>(null);
  const [licenseState, setLicenseState] = useState<LicenseState | null>(null);
  // B9 — Claude accounts dynamic list (for pane mode picker + badges)
  const [claudeAccounts, setClaudeAccounts] = useState<ClaudeAccount[]>([]);
  // 006 agent-profiles — agentes guardados (built-in + del user). reloadAgents tras CRUD.
  const [agents, setAgents] = useState<AgentProfile[]>([]);
  const [agentGalleryOpen, setAgentGalleryOpen] = useState(false);
  const reloadAgents = useCallback(() => {
    invoke<AgentProfile[]>("agent_profile_list").then(setAgents).catch(() => setAgents([]));
  }, []);
  // 008 orchestration — board + lanzamiento de una tarea en un pane on-demand.
  const [orchOpen, setOrchOpen] = useState(false);
  const launchOrchTask = useCallback(async (taskId: string) => {
    // backend: claim atómico + worktree (NO spawnea); el Terminal del pane spawnea.
    const info = await invoke<{ pane_id: string; worktree_path: string; mode: string; agent_profile_id: string | null; objective: string; session: string }>(
      "orchestration_prepare_task", { taskId },
    );
    const newPane: PaneCfg = {
      id: info.pane_id,
      mode: (info.agent_profile_id ? "zsh" : (info.mode || "zsh")) as PaneMode,
      title: "orch",
      cwd: info.worktree_path,
      kind: "terminal",
      agent_profile_id: info.agent_profile_id ?? undefined,
      orch_session: info.session, // sesión tmux única por tarea
    };
    setPanes((p) => (p.some((x) => x.id === info.pane_id) ? p : [...p, newPane]));
    navigate("panes");
    setFocusedPane(info.pane_id);
    setOrchOpen(false);
    // entregar el objetivo al agente una vez montado/spawneado el pane (Codex MED#5).
    if (info.objective.trim()) {
      window.setTimeout(() => {
        invoke("pty_write", { paneId: info.pane_id, data: info.objective + "\n", actionId: null, correlationId: `orch-${info.pane_id}` }).catch(() => {});
      }, 1800);
    }
  }, []);
  const [voiceOpen, setVoiceOpen] = useState(false);
  const [auditDrawerOpen, setAuditDrawerOpen] = useState(false);
  // 047 FR-004 — card desde la que se abrió el drawer de audit (su evento se resalta + su grupo se
  // expande). null = abierto sin foco en una card concreta.
  const [auditHighlightCardId, setAuditHighlightCardId] = useState<string | null>(null);
  const [auditFilter, setAuditFilter] = useState("");
  const [paletteMode, setPaletteMode] = useState<PaletteMode | null>(null);
  const [broadcastOpen, setBroadcastOpen] = useState(false);
  const [smartPasteOpen, setSmartPasteOpen] = useState(false);
  const [councilOpen, setCouncilOpen] = useState(false);
  // 016 US2/US3 — Help Center (con sección contextual) + What's New. Detrás de sus flags.
  const [helpOpen, setHelpOpen] = useState(false);
  const [helpSection, setHelpSection] = useState<string | undefined>(undefined);
  const [whatsNewOpen, setWhatsNewOpen] = useState(false);
  const [tourActive, setTourActive] = useState(false);
  const [tourOffered, setTourOffered] = useState(false);
  const [helpEnabled] = useFlag("helpCenter");
  const [whatsNewEnabled] = useFlag("whatsNew");
  const [toursEnabled] = useFlag("tours");
  const openHelp = useCallback((section?: string, source: "palette" | "topbar" | "deeplink" = "deeplink") => {
    if (!helpEnabled) return;
    setHelpSection(section);
    setHelpOpen(true);
    // 016 US5 — telemetry opt-in: SÓLO la fuente de apertura (allowlisted). Gate interno (OFF default).
    trackEvent("help_opened", { source });
  }, [helpEnabled]);
  // 015 T013/T031 — navegador interno: deep-links `furx://…`. Vistas → setView (+ scroll a sección
  // de Settings); modales potentes → onOpenModal (mapea nombre → su setter, sin tocar la vista).
  // 016 — Help/What's New → sus overlays. Definido acá (no antes) porque mapea los setters de arriba.
  const navigateInternal = useMemo(
    () =>
      createNavigator(
        // nav normal (deeplinks `furx://`, palette `view.*`/`nav.*`) → one-shot: limpia el filtro.
        (v) => navigate(v),
        (id) => {
          const el = document.getElementById(id);
          if (!el) return false;
          el.scrollIntoView({ block: "start", behavior: "smooth" });
          return true;
        },
        (modal) => {
          if (modal === "agents") setAgentGalleryOpen(true);
          else if (modal === "orchestration") setOrchOpen(true);
          else if (modal === "council") setCouncilOpen(true);
          else if (modal === "voice") setVoiceOpen(true);
        },
        (section) => openHelp(section),
        () => { if (whatsNewEnabled) setWhatsNewOpen(true); },
      ),
    [openHelp, whatsNewEnabled],
  );
  // 016 US4 — primer arranque: OFRECER (no forzar) el tour. Sólo si el flag está ON y nunca se ofreció
  // (persistido). Si el usuario lo descarta, marcamos firstRun done (no vuelve a aparecer solo). FR-014.
  useEffect(() => {
    if (toursEnabled && !isFirstRunDone()) setTourOffered(true);
  }, [toursEnabled]);
  // 016 US5 — primer fetch de la config de telemetry (opt-in + endpoint). Sin esto, el primer
  // trackEvent del arranque haría el await; precalentarla evita ese costo y mantiene el gate listo.
  useEffect(() => { void refreshTelemetryConfig(); }, []);
  // 042 FR-001 — spinner del boot con delay ANTI-FLASH (~150ms): si settings carga rapidísimo no
  // mostramos el spinner (evita un parpadeo). Sólo aparece si seguimos en `loading` pasado el delay.
  const [showBootSpinner, setShowBootSpinner] = useState(false);
  useEffect(() => {
    if (settingsState !== "loading") { setShowBootSpinner(false); return; }
    const t = setTimeout(() => setShowBootSpinner(true), 150);
    return () => clearTimeout(t);
  }, [settingsState]);
  // 042 FR-003 — banner "inferencia no configurada". Tras cargar settings, resolvemos el endpoint AIE
  // (settings o default localhost) y lo pingueamos con el MISMO health-check del wizard (1500ms, sin
  // redirect). Si NO responde, mostramos un banner discreto NO bloqueante (la app funciona sin AIE,
  // sólo sin las features de inferencia) con link a Ajustes→Servicios. Dismissible por sesión.
  const [infraUnreachable, setInfraUnreachable] = useState(false);
  const [infraBannerDismissed, setInfraBannerDismissed] = useState(false);
  useEffect(() => {
    if (settingsState !== "loaded") return;
    let cancelled = false;
    (async () => {
      try {
        // Endpoint AIE configurado (vacío → el backend lo trata como default localhost). Si esta
        // lectura falla, dejamos que el catch externo NO muestre el banner (un fallo de settings_get
        // no debe concluir nada sobre el estado de inferencia → evita un falso positivo).
        const aie = await invoke<unknown>("settings_get", { key: "endpoints.aie" });
        const aieUrl = typeof aie === "string" && aie.trim() ? aie.trim() : "http://localhost:8250";
        // 053 fix — el banner "inferencia no configurada" también debe contar OLLAMA, no solo AIE
        // (antes pasaba ollamaUrl:"" → con Ollama local corriendo pero AIE caído, el banner mentía).
        const oll = await invoke<unknown>("settings_get", { key: "endpoints.ollama" });
        const ollamaUrl = typeof oll === "string" && oll.trim() ? oll.trim() : "http://localhost:11434";
        const pair = await invoke<{ aie: { reachable: boolean }; ollama: { reachable: boolean } }>("setup_health_check", {
          aieUrl,
          ollamaUrl,
        });
        let ollamaOk = pair.ollama.reachable;
        // Si el Ollama configurado (p.ej. un server remoto) no responde, probar el LOCAL: una
        // inferencia local disponible alcanza para ocultar el banner.
        if (!ollamaOk && ollamaUrl !== "http://localhost:11434") {
          try {
            const localPair = await invoke<{ ollama: { reachable: boolean } }>("setup_health_check", {
              aieUrl: "",
              ollamaUrl: "http://localhost:11434",
            });
            ollamaOk = localPair.ollama.reachable;
          } catch { /* ignorar: si el 2º check falla, queda el resultado del 1º */ }
        }
        // Banner SOLO si NINGUNA inferencia (ni AIE ni Ollama) está disponible.
        if (!cancelled) setInfraUnreachable(!pair.aie.reachable && !ollamaOk);
      } catch {
        // El health-check falló (no el endpoint, sino el comando): no afirmamos "no configurado"
        // para no molestar con un falso positivo — dejamos el banner oculto.
        if (!cancelled) setInfraUnreachable(false);
      }
    })();
    return () => { cancelled = true; };
  }, [settingsState]);
  const [standupOpen, setStandupOpen] = useState(false);
  const [prModalOpen, setPrModalOpen] = useState(false);
  const [disagreeOpen, setDisagreeOpen] = useState(false);
const [shortcutSheetOpen, setShortcutSheetOpen] = useState(false);
// 047 FR-005 — Focus Mode: ⌘⇧F lleva el pane focado a ocupar toda la ventana (oculta sidebar,
// top bar y los demás panes); Esc sale. NO confundir con ⌘/ (atajos) ni ⌘P (búsqueda).
const [focusModeOn, setFocusModeOn] = useState(false);
// US2 (spec 015) — Command Palette ⌘K universal. Estado window-scoped (local,
// NO singleton): cada ventana tiene su palette. Convive con la palette legacy
// hasta la migración de nav (US10).
const [cmd015Open, setCmd015Open] = useState(false);
const [tmuxAvailable, setTmuxAvailable] = useState<boolean>(true);
const [homeDir, setHomeDir] = useState<string>("");
  const [restoreSessions, setRestoreSessions] = useState<FurxSession[] | null>(null);
  const [pendingSuggestion, setPendingSuggestion] = useState<{ paneId: string; paneTitle: string; action: SuggestionAction } | null>(null);
  // BLOQUE A · J: pane-buffer + suggestion polling lives in a dedicated hook.
  const { captureOutput, paneSuggestions, bufferOf, snapshotBuffers, forgetPane } = usePaneBuffers();
  // spec 001 US1 — TTS: probe local OS engine once; read focused pane's buffer aloud.
  const [ttsAvailable, setTtsAvailable] = useState(false);
  useEffect(() => { invoke<boolean>("tts_available").then(setTtsAvailable).catch(() => setTtsAvailable(false)); }, []);

  // 017 mobile-companion nav — (1) push the materialized NavSpec to the bridge so
  // the phone renders the bottom-nav from the SSOT (navGroups), and (2) handle
  // exec requests routed from the phone (`furx:mobile-exec`) by invoking the
  // command through the SAME gated invoke wrapper (universal approval gate). The
  // bridge already re-authorized at exec-time; this is the actual dispatch path.
  useEffect(() => {
    // SSOT push: buildNavSpec() materializes the curated subset from navGroups, with labels
    // resolved through the SAME i18n catalog as the desktop (audit MED 2 — no es↔mobile divergence).
    // `t` in deps → on locale change the NavSpec is re-pushed with the translated labels.
    const navSpec = buildNavSpec((key) => t(key));
    invoke("mobile_bridge_set_navspec", { spec: navSpec }).catch(() => {
      // Non-fatal: bridge may not be up yet (started ~2s after window). Retry once.
      setTimeout(() => {
        invoke("mobile_bridge_set_navspec", { spec: navSpec }).catch(() => {});
      }, 3000);
    });
    let un: (() => void) | null = null;
    listen<{ command_id: string }>("furx:mobile-exec", (e) => {
      const id = e.payload?.command_id;
      if (!id) return;
      // Re-invoke through the gated wrapper. Zero-arg in this corte (T068): risky
      // commands already came back as pending_approval from the bridge; a Safe
      // command runs here through the universal gate (no mobile-only shortcut).
      invoke(id).catch(() => {
        // Pending-approval rejects here too (front re-invokes on approval) — silent.
      });
    }).then((u) => { un = u; }).catch(() => {});
    return () => { if (un) un(); };
  }, [t]);
  const readAloud = useCallback(() => {
    if (!focusedPane) return;
    const text = bufferOf(focusedPane) || "";
    invoke("tts_speak", { paneId: focusedPane, text, summarize: true, preempt: true }).catch(() => {});
  }, [focusedPane, bufferOf]);
  const stopReadAloud = useCallback(() => { invoke("tts_stop").catch(() => {}); }, []);
  // spec 001 T032 — auto-read-on-idle. Per-pane opt-in (default OFF, persisted).
  // A pane is "idle/done" when its buffer is non-empty and unchanged across one
  // ~1.2s tick (debounce). On idle+changed we read the heuristic summary; the Rust
  // mutex drops the request if another pane is already speaking (preempt:false).
  const [autoReadPanes, setAutoReadPanes] = useState<Set<string>>(() => {
    try { return new Set(JSON.parse(localStorage.getItem("furx-autoread") || "[]")); } catch { return new Set(); }
  });
  const toggleAutoRead = useCallback((paneId: string) => {
    setAutoReadPanes((prev) => {
      const next = new Set(prev);
      if (next.has(paneId)) next.delete(paneId); else next.add(paneId);
      try { localStorage.setItem("furx-autoread", JSON.stringify([...next])); } catch { /* ignore */ }
      return next;
    });
  }, []);
  const autoReadState = useRef<Record<string, { seen: string; read: string }>>({});
  useEffect(() => {
    if (!ttsAvailable || autoReadPanes.size === 0) return;
    const id = window.setInterval(() => {
      for (const paneId of autoReadPanes) {
        const buf = bufferOf(paneId) || "";
        const st = autoReadState.current[paneId] || { seen: "", read: "" };
        if (buf && buf === st.seen && buf !== st.read) {
          // idle (stable) + changed since last read → read once; backend mutex
          // drops it if another pane is speaking (preempt:false).
          invoke("tts_speak", { paneId, text: buf, summarize: true, preempt: false }).catch(() => {});
          autoReadState.current[paneId] = { seen: buf, read: buf };
        } else {
          autoReadState.current[paneId] = { seen: buf, read: st.read };
        }
      }
    }, 1200);
    return () => window.clearInterval(id);
  }, [ttsAvailable, autoReadPanes, bufferOf]);
  // Declared before the PTT effect so `settle` can surface failures (mic permission,
  // missing sox/whisper) via toast instead of swallowing them. Stable (useCallback).
  const { toasts, show: showToast, dismiss: dismissToast } = useToast();
  // spec 005 — push-to-talk: hold el hotkey configurable (default ⌥Space) → record while held →
  // release → transcribe (Whisper local) → write to the pane focused AT START. Esc/blur cancels.
  const [pttRecording, setPttRecording] = useState(false);
  // 059 — hotkey de PTT configurable (Ajustes → Atajos). Se carga de settings (`ptt.hotkey`) al montar
  // y se actualiza en vivo cuando Ajustes lo cambia (CustomEvent), sin reiniciar. Default ⌥Space.
  // El `pttHotkeyRef` (config ACTUAL parseada) lo leen los handlers del PTT SIN re-bindear los
  // listeners (audit: re-bindear por deps perdía el keyup en vuelo). `activeHkRef` captura el combo
  // que INICIÓ la grabación en curso → un cambio de hotkey mid-grabación no deja el release huérfano.
  const [pttHotkey, setPttHotkey] = useState<string>(DEFAULT_PTT_HOTKEY);
  const pttHotkeyRef = useRef(parsePttHotkey(DEFAULT_PTT_HOTKEY));
  const activeHkRef = useRef<ReturnType<typeof parsePttHotkey> | null>(null);
  useEffect(() => { pttHotkeyRef.current = parsePttHotkey(pttHotkey); }, [pttHotkey]);
  useEffect(() => {
    invoke<unknown>("settings_get", { key: "ptt.hotkey" })
      .then((v) => { if (typeof v === "string" && v.trim()) setPttHotkey(v.trim()); })
      .catch(() => { /* sin override → default */ });
    const onChange = (e: Event) => {
      const v = (e as CustomEvent<string>).detail;
      if (typeof v === "string" && v.trim()) setPttHotkey(v.trim());
    };
    window.addEventListener("furx:ptt-hotkey", onChange);
    return () => window.removeEventListener("furx:ptt-hotkey", onChange);
  }, []);
  // 021-voice-es — el pane destino se CONGELA al iniciar; el indicador se ancla a ESE pane
  // (no al focuseado) y el texto se inserta ahí aunque cambie el foco.
  const [recordingPane, setRecordingPane] = useState<string | null>(null); // pane destino congelado → glow + ancla
  const [voiceState, setVoiceState] = useState<VoiceState>("idle"); // idle | recording | transcribing
  // 055 wedge — foregrounding de 2 diferenciales en la barra (PTT ya tiene su indicador de voz):
  // (1) Móvil: estado del bridge para el CTA "continuá en el teléfono" en el área de marca.
  const [mobileRunning, setMobileRunning] = useState(false);
  useEffect(() => {
    invoke<{ running: boolean }>("mobile_bridge_status")
      .then((st) => setMobileRunning(!!st?.running))
      .catch(() => setMobileRunning(false)); // best-effort: sin estado, el CTA igual abre Ajustes→Móvil
  }, []);
  // (2) Memoria: count de entradas para el badge del ítem de espina (la memoria persistente "viva").
  const [memoryCount, setMemoryCount] = useState<number | null>(null);
  useEffect(() => {
    invoke<{ total_entries: number }>("memory_stats")
      .then((s) => setMemoryCount(Number.isFinite(s?.total_entries) && s.total_entries >= 0 ? s.total_entries : null))
      .catch(() => setMemoryCount(null));
  }, []);
  // Sync state (refs) so key-repeat can't double-start before the async start resolves.
  const ptt = useRef<{ active: boolean; released: boolean; canceled: boolean; session: { id: string; pane: string | null } | null }>(
    { active: false, released: false, canceled: false, session: null }
  );
  const focusedPaneRef = useRef<string | null>(null);
  focusedPaneRef.current = focusedPane;
  // F1/F2 — la lista ACTUAL de panes, accesible dentro del effect del PTT sin re-subscribir
  // los listeners en cada cambio de panes. Se usa para verificar que el pane destino congelado
  // SIGUE existiendo antes de insertar (F1) y para anclar/repostar el indicador (F2).
  const panesRef = useRef<PaneCfg[]>(panes);
  panesRef.current = panes;
  // F4 — cache del whisper_check (readiness del modelo). Se refresca al montar y tras cada
  // intento fallido de PTT. Si el modelo no está listo (ready==false / needs_migration),
  // NO grabamos: mostramos un hint accionable en vez de fallar muda.
  const whisperReadyRef = useRef<{ ready: boolean; needs_migration: boolean; install_hint: string } | null>(null);
  const refreshWhisper = useCallback(() => {
    invoke<{ ready: boolean; needs_migration: boolean; install_hint: string }>("whisper_check")
      .then((c) => { whisperReadyRef.current = c; })
      .catch(() => { whisperReadyRef.current = null; }); // unknown → no bloqueamos por un check fallido
  }, []);
  useEffect(() => { refreshWhisper(); }, [refreshWhisper]);
  // Race-guard (deepseek): id de la última captura iniciada. Una transcripción vieja sólo
  // inserta si su captura sigue siendo la vigente (o si ya no hay ninguna en curso); así una
  // grabación nueva iniciada mientras otra transcribe no mete texto en el pane equivocado.
  const lastSessionIdRef = useRef<string | null>(null);
  useEffect(() => {
    // 059 — hotkey configurable: los handlers leen `pttHotkeyRef.current` (config actual) → este bind
    // es ESTABLE (NO depende de `pttHotkey`), así un cambio de hotkey NO re-registra listeners ni pierde
    // un keyup en vuelo (audit codex/deepseek/aie). `activeHkRef` guarda el combo que inició la grabación.
    const reset = () => { ptt.current = { active: false, released: false, canceled: false, session: null }; setPttRecording(false); setRecordingPane(null); setVoiceState("idle"); };
    const settle = (id: string, pane: string | null, cancel: boolean) => {
      // Idempotent: releasing a held ⌥Space fires TWO keyups (Space, then Alt) and both
      // match the keyup guard. Clear the session up front so the 2nd can't re-stop the
      // same capture — that would `no such capture`-error and pop a false toast now that
      // errors are surfaced.
      if (ptt.current.session?.id === id) ptt.current.session = null;
      if (cancel) { invoke("voice_ptt_cancel", { id }).catch(() => {}); reset(); return; }
      // 021-voice-es — soltó la tecla: grabación → transcribiendo (ancla del pane destino lo refleja).
      setVoiceState("transcribing");
      // Race-guard (deepseek): ESTE id es ahora la captura vigente. Si arranca otra grabación
      // mientras transcribimos, `lastSessionIdRef.current` cambia y esta transcripción vieja NO
      // resetea el estado de la nueva ni inserta en su pane.
      const isCurrent = () => lastSessionIdRef.current === id;
      // F3 — sólo reseteamos a idle si seguimos siendo la captura vigente; si ya empezó otra,
      // dejamos su estado intacto. Cualquier error path llega acá → nunca queda "transcribing" colgado.
      const settleReset = () => { if (isCurrent()) reset(); };
      // F1 — insertar el texto SIN pérdida silenciosa: si el pane destino congelado ya no
      // existe (se cerró mientras grababa/transcribía), no lo tragamos. Fallback: pane focuseado
      // actual si existe; si tampoco, toast con la transcripción para que el usuario la recupere.
      const deliver = async (text: string) => {
        if (!isCurrent()) return; // otra grabación tomó el control; no insertamos texto viejo
        if (!text) return;
        // 030 F0-wire — ¿la transcripción es un COMANDO DE FOCO de voz ("siguiente" / "andá a X" /
        // "quién me necesita")? Si sí, NAVEGAMOS (no dictamos). El backend acuña el witness humano y
        // mueve el foco SÓLO si el nombre resuelve a un pane vivo. Si devuelve null → es dictado normal.
        try {
          const refPanes = panesRef.current.map((p) => ({
            pane_id: p.id,
            label: p.title || p.mode,
            // alias por mode + mode-sin-guiones ("claude-A" → "claude a") + title, para el match por voz.
            aliases: [p.mode, p.mode.replace(/-/g, " "), p.title].filter(Boolean) as string[],
          }));
          const outcome = await invoke<{ kind: string; value?: unknown } | null>(
            "attention_command",
            { transcript: text, panes: refPanes },
          );
          // Race-guard (audit codex front): RE-chequear tras el await — si arrancó otra grabación
          // mientras consultábamos al backend, esta transcripción vieja NO debe navegar ni dictar.
          if (!isCurrent()) return;
          if (outcome) {
            const labelOf = (pid: string) => panesRef.current.find((p) => p.id === pid)?.title || pid;
            if (outcome.kind === "focused" && typeof outcome.value === "string") {
              setFocusedPane(outcome.value);
              showToast("info", `Foco → ${labelOf(outcome.value)}`, 3000);
            } else if (outcome.kind === "no_match") {
              showToast("info", `No encontré "${String(outcome.value ?? "")}"`, 4000);
            } else if (outcome.kind === "queue_empty") {
              showToast("info", "No hay panes esperándote", 3000);
            } else if (outcome.kind === "listed") {
              const arr = Array.isArray(outcome.value) ? (outcome.value as Array<{ pane_id: string }>) : [];
              showToast("info", arr.length ? `Te necesitan: ${arr.map((e) => labelOf(e.pane_id)).join(", ")}` : "Nadie te necesita ahora", 5000);
            } else if (outcome.kind === "silenced") {
              // 032 U2 — "callar" por voz: el audio de avisos se silenció (no se movió el foco).
              showToast("info", "Avisos de audio silenciados", 3000);
            } else if (outcome.kind === "reading_result") {
              // 032 U3 — leyendo el resultado del pane nombrado (resumen redactado, no el crudo).
              showToast("info", `Leyendo el resultado de ${String(outcome.value ?? "")}`, 3000);
            } else if (outcome.kind === "no_result") {
              showToast("info", `${String(outcome.value ?? "Ese pane")} no tiene resultado para leer`, 4000);
            }
            if (isCurrent()) reset();
            return; // fue un comando de foco, NO dictado
          }
        } catch {
          // si el comando de atención falla, no rompemos el dictado: seguimos al path normal.
          // pero re-chequeamos el race-guard tras el await fallido (audit codex front).
          if (!isCurrent()) return;
        }
        const paneIds = panesRef.current.map((p) => p.id);
        const destAlive = !!pane && paneIds.includes(pane);
        const fallbackPane = focusedPaneRef.current;
        const target = destAlive ? pane : (fallbackPane && paneIds.includes(fallbackPane) ? fallbackPane : null);
        if (!target) {
          // No hay ningún pane vivo donde escribir → mostramos el texto para que no se pierda.
          showToast("info", `Transcripción (sin pane destino): ${text}`, 12000);
          return;
        }
        invoke("pty_write", { paneId: target, data: text, actionId: null, correlationId: `ptt-${id}` })
          .then(() => {
            // Si el destino congelado se cerró y caímos al focuseado, avisamos del redireccionamiento.
            if (!destAlive) showToast("info", "El pane destino se cerró; texto insertado en el pane activo.", 5000);
          })
          .catch((e) => {
            // NO tragar el error del pty_write: el pane pudo morir entre el check y la escritura.
            // El texto no se pierde — lo mostramos en un toast para copiarlo.
            showToast("error", `No se pudo insertar el dictado (${String(e || "")}). Texto: ${text}`, 12000);
          });
      };
      invoke<{ path: string }>("voice_ptt_stop", { id })
        .then((r) => invoke<{ text: string }>("voice_transcribe", { audioPath: r.path }))
        // RETORNAR la promesa de `deliver` (ahora async) para que el `.finally(settleReset)` espere a
        // que termine el guard/fallback completo — sino settleReset resetearía el estado a mitad del
        // await de `attention_command` (audit codex front: cierra el race de la transcripción async).
        .then((t) => deliver((t.text || "").trim()))
        .catch((e) => {
          // Surface the real reason instead of dropping it. The most common failure is
          // silent capture (mic permission denied → CoreAudio returns silence): the
          // backend returns `no_audio`, since whisper hallucinates "you" on empty audio.
          // F3: este catch cubre stop/transcribe fallido, descarga fallida, mic desconectado
          // a mitad y timeout — todos resetean a idle vía settleReset() en .finally.
          if (!isCurrent()) return; // error de una captura ya superada → silencioso para no confundir
          const msg = String(e || "");
          if (/no_audio/i.test(msg)) {
            showToast("error", "No se detectó audio. Revisá el permiso de micrófono: Ajustes del Sistema → Privacidad y seguridad → Micrófono → Furx.", 8000);
          } else if (/not in PATH|sox|whisper/i.test(msg)) {
            showToast("error", `Voz no disponible — ${msg}`, 8000);
          } else {
            showToast("error", `Push-to-talk falló: ${msg}`, 6000);
          }
          refreshWhisper(); // un fallo puede deberse al modelo: re-chequeamos readiness para F4
        })
        .finally(settleReset);
    };
    const inEditable = () => {
      const el = document.activeElement as HTMLElement | null;
      if (!el) return false;
      // xterm's hidden helper textarea holds pane focus — PTT MUST work there
      // (that's the whole point: dictate INTO the focused terminal). Don't treat it
      // as a text input; only real inputs (palette, modals) block PTT.
      if (el.classList?.contains("xterm-helper-textarea") || el.closest?.(".xterm, .terminal-host")) return false;
      return el.tagName === "INPUT" || el.tagName === "TEXTAREA" || el.isContentEditable;
    };
    // F4 — decide si grabar según la readiness del modelo. Devuelve true si hay que ABORTAR
    // (mostró el hint accionable). `wc` puede venir del cache o de un check fresco.
    const blockedByReadiness = (wc: { ready: boolean; needs_migration: boolean; install_hint: string } | null): boolean => {
      if (wc && !wc.ready) {
        const hint = wc.needs_migration
          ? "El modelo de voz cambió a multilingüe. Descargalo en Ajustes (⌘⇧V)."
          : "El modelo de voz no está instalado. Descargalo en Ajustes (⌘⇧V).";
        showToast("error", hint, 8000);
        refreshWhisper(); // por si cambió desde el último check
        return true;
      }
      return false;
    };
    // Begin a PTT capture for `pane`. Shared by the ⌥Space and Cmd/Win-hold triggers.
    // async: el PRIMER PTT con cache desconocido (null) hace un whisper_check fresco antes de
    // grabar, para cerrar la carrera de readiness (F4). PTTs siguientes ya tienen el cache → sin
    // latencia extra. El keyup durante el await sólo setea st.released (st.session sigue null), así
    // que el .then de voice_ptt_start lo detecta y settlea sin dejar una grabación huérfana.
    const startPtt = async () => {
      const st = ptt.current;
      clearMeta(); // cancel any pending Meta-hold timer (this trigger won)
      if (st.active) return; // anti key-repeat / double-start (cubre también el await del check fresco)
      // Marcamos active YA, antes de cualquier await: bloquea un 2º startPtt durante el check fresco.
      // Si abortamos (modelo no listo / check fallido) llamamos reset() para liberar el flag.
      st.active = true; st.released = false; st.canceled = false; st.session = null;
      // F4 — resolver la carrera de readiness ANTES de grabar.
      // Cache conocido (ready==false / needs_migration) → abortar con hint, sin grabar.
      let wc = whisperReadyRef.current;
      if (wc === null) {
        // Cache desconocido (whisper_check aún no resolvió o falló): hacemos un check fresco
        // (local, rápido) y decidimos con ESE resultado en vez de grabar a ciegas.
        try {
          wc = await invoke<{ ready: boolean; needs_migration: boolean; install_hint: string }>("whisper_check");
          whisperReadyRef.current = wc; // poblar cache para los PTTs siguientes
        } catch (e) {
          // El check mismo falló: en vez de grabar a ciegas (el fallback F3 surfacearía el error
          // recién post-transcripción), avisamos y abortamos — más robusto y sin colgar la UI.
          showToast("error", `No se pudo verificar el modelo de voz (${String(e || "")}). Reintentá o descargalo en Ajustes (⌘⇧V).`, 8000);
          reset();
          return;
        }
      }
      if (blockedByReadiness(wc)) { reset(); return; }
      // Si el usuario soltó la tecla durante el await (keyup ya seteó st.released con st.session
      // null), NO arrancamos una grabación huérfana: era un tap demasiado corto para grabar.
      if (st.released || st.canceled) { reset(); return; }
      const pane = focusedPaneRef.current; // 021-voice-es — destino CONGELADO acá; no sigue al foco
      setPttRecording(true); setRecordingPane(pane); setVoiceState("recording"); // pane destino: glow + ancla
      invoke("tts_stop").catch(() => {}); // voice-interrupt
      invoke<string>("voice_ptt_start")
        .then((id) => {
          // Race-guard: marcamos esta captura como la vigente apenas tenemos su id; una nueva
          // grabación posterior la reemplaza y la vieja deja de insertar/resetear (ver settle).
          lastSessionIdRef.current = id;
          if (st.canceled) settle(id, pane, true);
          else if (st.released) settle(id, pane, false);
          else st.session = { id, pane };
        })
        .catch((e) => {
          // F3 — el arranque falló: reset a idle + toast (nunca queda colgado en "recording").
          showToast("error", `Push-to-talk no pudo iniciar: ${String(e || "")}`, 6000);
          reset();
          refreshWhisper(); // pudo ser el modelo
        });
    };
    // Cmd (mac) / Win (Meta) held ALONE → PTT. Guard: any other key while Meta is
    // down means it's a shortcut (⌘C…) → abort. A short threshold avoids firing on
    // the brief Meta-down before a shortcut letter.
    const META_HOLD_MS = 350;
    let metaTimer: number | null = null;
    let metaShortcut = false;
    const clearMeta = () => { if (metaTimer !== null) { window.clearTimeout(metaTimer); metaTimer = null; } };
    const onKeyDown = (e: KeyboardEvent) => {
      // 059 — si Ajustes está GRABANDO un hotkey nuevo, no disparar PTT (sino el combo grabaría voz).
      if (isPttCapturing()) return;
      const st = ptt.current;
      if (matchesPttHotkey(e, pttHotkeyRef.current) && !st.active && !inEditable()) {
        activeHkRef.current = pttHotkeyRef.current; // captura el combo que inicia ESTA grabación
        e.preventDefault();
        startPtt();
      } else if (e.key === "Meta" && !e.repeat && !st.active && !inEditable()) {
        // Meta pressed alone — arm a timer; if held alone past the threshold, record.
        metaShortcut = false;
        clearMeta();
        metaTimer = window.setTimeout(() => { if (!metaShortcut) startPtt(); }, META_HOLD_MS);
      } else if (e.key !== "Meta" && (e.metaKey || metaTimer !== null) && !st.active) {
        // Another key while Meta is down → it's a shortcut, not PTT.
        metaShortcut = true;
        clearMeta();
      } else if (e.key === "Escape" && st.active) {
        st.canceled = true;
        if (st.session) settle(st.session.id, st.session.pane, true);
      }
    };
    const onKeyUp = (e: KeyboardEvent) => {
      const st = ptt.current;
      if (e.key === "Meta") {
        clearMeta(); // released before threshold → no recording (or stop if it started)
      }
      // settle al soltar la tecla base del combo QUE INICIÓ esta grabación (activeHkRef), cualquiera de
      // sus modificadores, o Meta (gesto Cmd-hold). Usar el combo capturado (no la config actual) hace
      // que un cambio de hotkey mid-grabación no deje el release huérfano. settle idempotente → los dos
      // keyup del held-key (tecla + modificador) son OK.
      const ahk = activeHkRef.current;
      if (st.active && ((ahk && (e.code === ahk.code || pttModifierKeyNames(ahk).includes(e.key))) || e.key === "Meta")) {
        st.released = true;
        if (st.session) settle(st.session.id, st.session.pane, false); // else start.then sends on resolve
      }
    };
    const onCancelBlur = () => {
      clearMeta(); // a pending Meta-hold timer must not fire after we lose focus
      const st = ptt.current;
      if (!st.active) return;
      st.canceled = true;
      if (st.session) settle(st.session.id, st.session.pane, true); // else start.then cancels
    };
    const onVisibility = () => { if (document.hidden) onCancelBlur(); };
    // Capture phase so xterm (which handles keydown on its textarea) can't swallow
    // the PTT key before we see it.
    window.addEventListener("keydown", onKeyDown, true);
    window.addEventListener("keyup", onKeyUp, true);
    window.addEventListener("blur", onCancelBlur);
    document.addEventListener("visibilitychange", onVisibility);
    return () => {
      clearMeta(); // no orphan timer firing startPtt after unmount
      window.removeEventListener("keydown", onKeyDown, true);
      window.removeEventListener("keyup", onKeyUp, true);
      window.removeEventListener("blur", onCancelBlur);
      document.removeEventListener("visibilitychange", onVisibility);
    };
  }, [showToast, refreshWhisper]);
  // BLOQUE B · Codex audit MED #3: idle detection needs a minimum age since
  // spawn so we don't promote a freshly-mounted pane whose buffer is empty
  // only because its prompt hasn't echoed yet. Track per-pane epoch on add.
  const paneBornAt = useRef<Record<string, number>>({});
  const MIN_IDLE_AGE_MS = 5000;
  // BLOQUE C · F7 — open inter-pane send modal with this source pane.
  const [interpaneSource, setInterpaneSource] = useState<PaneCfg | null>(null);
  // BLOQUE G · F8 — when the merge_watcher fires, surface a MergeReview modal
  // with the diff stat + risky paths. payload comes from the watcher event.
  const [mergeReview, setMergeReview] = useState<{ repoPath: string; branch: string } | null>(null);
  // BLOQUE D · F12 auto-poll: dedup by content hash so a single copy can't
  // re-toast every second. We keep the last 8 hashes (cheap LRU).
  const recentPasteHashes = useRef<string[]>([]);
  // BLOQUE A · G: snapshot in-flight guard so a held ⌘⇧S can't pile up requests.
  const snapshotInFlight = useRef<boolean>(false);
  const councilCooldown = useRef<number>(0);
  const [usage, setUsage] = useState<UsageSummary | null>(null);
  const [aieState, setAieState] = useState<AieState | null>(null);
  const [usageStaleAt, setUsageStaleAt] = useState<number>(Date.now());
  const [aieStaleAt, setAieStaleAt] = useState<number>(Date.now());
  const layoutLoaded = useRef(false);

  // Load layout + first-run check at mount.
  useEffect(() => {
    // 042 FR-001 — guardas de desmontaje del boot: el timer de 8s y el callback tardío del
    // `allSettled` NO deben actualizar estado tras desmontar (ni dejar el timer colgado). Se limpian
    // en el cleanup de abajo. `bootTimer` se asigna dentro del bloque y se referencia acá.
    let bootCancelled = false;
    let bootTimer: ReturnType<typeof setTimeout> | undefined;
    invoke<LayoutFromDB | null>("get_layout", { id: "default" })
      .then((l) => {
        if (l) {
          setPanes(l.panes);
          setGridCols(l.grid_cols || "1fr 1fr");
          setGridRows(l.grid_rows || "1fr 1fr");
        }
        layoutLoaded.current = true;
      })
      .catch(() => { layoutLoaded.current = true; });
    // 042 FR-001 — el gate del wizard depende de DOS invokes (first-run + tmux). Los corremos con
    // `Promise.allSettled` (NO `Promise.all`): si `tmux_available` falla transitoriamente (tmux no
    // instalado en la máquina del usuario), eso NO debe disparar el wizard ni tumbar el boot. Un
    // timeout DURO de 8s evita quedar colgado en el spinner si el backend no responde.
    {
      let settled = false;
      const finalize = (state: "loaded" | "error", wizard: boolean) => {
        // Una sola resolución (timeout XOR allSettled) Y nunca tras desmontar.
        if (settled || bootCancelled) return;
        settled = true;
        if (bootTimer !== undefined) clearTimeout(bootTimer);
        setSettingsState(state);
        setNeedsWizard(wizard);
      };
      bootTimer = setTimeout(() => {
        // No respondió en 8s → error + ir al wizard (salvo que el fallsafe local ya lo dé por hecho).
        const d = decideBootTimeout(firstRunCompletedLocal());
        finalize(d.settingsState, d.needsWizard);
      }, 8000);
      Promise.allSettled([
        invoke<unknown>("settings_get", { key: "app.first_run_completed" }),
        invoke<boolean>("tmux_available"),
      ]).then(([firstRunRes, tmuxRes]) => {
        if (bootCancelled) return; // desmontado mientras resolvía → no tocar estado
        // Lógica pura (lib/boot.decideBoot): tmux independiente del wizard; first_run + fallsafe local.
        const d = decideBoot(firstRunRes, tmuxRes, firstRunCompletedLocal());
        setTmuxAvailable(d.tmuxAvailable);
        finalize(d.settingsState, d.needsWizard);
      });
    }
    // Pro-active flag + license state (BLOQUE 1 — license gate)
    // HIGH-1 fix (Codex): fail-CLOSED. On error, proActive=false → Pro features stay gated,
    // which is the right call when we don't know if the trial is still valid.
    invoke<boolean>("license_is_pro")
      .then(setProActive)
      .catch(() => setProActive(false));
    invoke<LicenseState>("license_check")
      .then(setLicenseState)
      .catch(() => setLicenseState(null));
    // B9 — load claude accounts
    invoke<ClaudeAccount[]>("claude_accounts_list")
      .then(setClaudeAccounts)
      .catch(() => setClaudeAccounts([]));
    // 006 — load agent profiles (built-in + user)
    invoke<AgentProfile[]>("agent_profile_list")
      .then(setAgents)
      .catch(() => setAgents([]));
    // RESTORE-FIX 2026-05-26 — tmux availability (042 FR-001: ahora se resuelve junto al first-run
    // vía Promise.allSettled arriba, para que un fallo de tmux no dispare el wizard espurio).
    // AUDIT-HARDCODE 2026-05-26 — home dir as default for repo-path inputs.
    invoke<string>("home_dir")
      .then(setHomeDir)
      .catch(() => setHomeDir(""));
    // BLOQUE D · F20 — install `~/bin/spec` alias once. Best-effort: silently
    // skip if it already exists (idempotent) or fails (PATH issue surfaces in
    // the user's own shell via `command -v spec`, not via us).
    invoke<boolean>("spec_kit_install_alias")
      .then((installed) => {
        if (installed) console.info("furx: ~/bin/spec alias installed");
      })
      .catch((e) => console.warn("spec_kit_install_alias skipped:", e));
    // 042 FR-001 — cleanup: cancelar el gate del boot al desmontar (no state-update tardío, no timer
    // colgado). El resto de invokes del effect son fire-and-forget idempotentes (sus setState sobre un
    // árbol desmontado son no-ops benignos en React 19; el gate del boot sí se guarda por ser el que
    // controla la pantalla de carga).
    return () => {
      bootCancelled = true;
      if (bootTimer !== undefined) clearTimeout(bootTimer);
    };
  }, []);

  // Hot-reload claude accounts on changes (event-driven)
  useEffect(() => {
    let mounted = true;
    let unlisten: undefined | (() => void);
    (async () => {
      try {
        unlisten = await listen("claude-accounts:changed", () => {
          if (mounted) invoke<ClaudeAccount[]>("claude_accounts_list").then(setClaudeAccounts).catch((e) => { console.warn("claude_accounts_list refresh failed", e); });
        });
      } catch { /* ignore */ }
    })();
    return () => { mounted = false; if (unlisten) unlisten(); };
  }, []);

  // Persist layout on changes (debounced).
  useEffect(() => {
    if (!layoutLoaded.current) return;
    const t = setTimeout(() => {
      invoke("save_layout", {
        layout: {
          id: "default",
          name: "Default 2×2",
          panes,
          grid_cols: gridCols,
          grid_rows: gridRows,
        },
      }).catch(console.error);
    }, 800);
    return () => clearTimeout(t);
  }, [panes, gridCols, gridRows]);

  const refreshAll = useCallback(async () => {
    // 044 FR-002 — el fetch de cards distingue éxito/fallo para alimentar el skeleton/error/retry del
    // inbox. Usamos allSettled para que un fallo de list_cards NO tumbe el refresh de monitors/events
    // (y viceversa). Si list_cards falla, NO pisamos `cards` con [] (preservamos datos viejos en pantalla
    // y mostramos el banner de error con "Reintentar").
    const [cRes, mRes, eRes] = await Promise.allSettled([
      invoke<Card[]>("list_cards"),
      invoke<MonitorSnapshot[]>("list_monitors"),
      invoke<AuditEvent[]>("list_events", { limit: 100 }),
    ]);
    if (cRes.status === "fulfilled") {
      setCards(cRes.value);
      setCardsError(null);
    } else {
      setCardsError(String(cRes.reason));
    }
    setCardsLoading(false);
    if (mRes.status === "fulfilled") setMonitors(mRes.value);
    if (eRes.status === "fulfilled") setEvents(eRes.value);
    setLastRefreshAt(Date.now()); // 022 P0b — freshness barato de los stats del sidebar.
  }, []);

  // 047 FR-008 — refresco dirigido-por-eventos (flag OFF por default). ON: el intervalo baja a una red
  // de seguridad lenta (20s) y los pushes del event bus (más abajo) disparan refreshAll al instante.
  // OFF: el intervalo de 5s de siempre (cero regresión). Si el push falla, el intervalo lento cubre.
  const [orchSse] = useFlag("orchestrationSSE");
  useEffect(() => {
    refreshAll();
    const id = setInterval(refreshAll, orchSse ? 20000 : 5000);
    const unlisten = listen<MonitorResult>("monitor:result", (ev) => {
      setMonitors((prev) => prev.map((s) => s.target.id === ev.payload.id ? { ...s, last: ev.payload } : s));
    });
    return () => { clearInterval(id); unlisten.then((u) => u()); };
  }, [refreshAll, orchSse]);

  // 047 FR-008 — suscripción a los pushes del backend SÓLO cuando el flag está ON. Refresca incidentes/
  // atención/monitores al instante ante un cambio real (tarea/agente/comando). El handler es no-op si el
  // flag está OFF → cero efecto cuando no se opta-in. Es additivo al intervalo (no lo reemplaza): el
  // intervalo lento sigue como fallback si un push se pierde (fail-safe).
  useAppEvent("TaskChanged", () => { if (orchSse) void refreshAll(); });
  useAppEvent("AgentStateChanged", () => { if (orchSse) void refreshAll(); });
  useAppEvent("CommandExecuted", () => { if (orchSse) void refreshAll(); });

  // F5 + F15 — top-bar strips (polled every 30s via extracted hook).
  // F26 StaleWatch component (in TopBar) handles the "delay > 2× interval" UI.
  usePolling(async () => {
    try {
      const u = await invoke<UsageSummary>("claude_usage_summary");
      setUsage(u); setUsageStaleAt(Date.now());
    } catch { /* keep last value; StaleWatch will reflect age */ }
    try {
      const s = await invoke<AieState>("aie_state");
      setAieState(s); setAieStaleAt(Date.now());
    } catch { /* idem */ }
  }, { intervalMs: 30000 });

  // F9 / I — map a Suggestion kind → SuggestionAction (with explicit pty text).
  // Buffer capture + suggestion polling live in usePaneBuffers (PLAN_CLOSE J).
  const onSuggestionClick = useCallback((paneId: string, paneTitle: string, sug: Suggestion) => {
    let pty_text = "";
    switch (sug.kind) {
      case "merge-conflict": pty_text = "git status\n"; break;
      case "prompt": pty_text = ""; setFocusedPane(paneId); return;
      case "test-pass": pty_text = "git add -A && git commit -m 'tests pass'\n"; break;
      case "build-ok": pty_text = ""; setFocusedPane(paneId); return;
      case "error": {
        const buf = bufferOf(paneId);
        const tail = buf.split("\n").slice(-50).join("\n");
        pty_text = `Investigá este error:\n\n\`\`\`\n${tail}\n\`\`\`\n`;
        break;
      }
      default: pty_text = ""; setFocusedPane(paneId); return;
    }
    if (!pty_text) return;
    const action: SuggestionAction = { kind: sug.kind, label: sug.label, hint: sug.hint, pty_text };
    setPendingSuggestion({ paneId, paneTitle, action });
  }, [bufferOf]);

  // F — boot-restore event from lib.rs setup (only fires if FURX_* tmux sessions pre-exist).
  // 2026-05-27: changed default to SILENT AUTO-ATTACH (was always-modal).
  // - Default: silently invoke boot_restore_attach + emit toast.
  // - Opt-in modal: setting `restore.always_ask=true` keeps the legacy 3-button modal
  //   for users who want explicit control.
  useEffect(() => {
    let off: (() => void) | undefined;
    listen<{ sessions: FurxSession[] }>("furx:boot-restore", async (ev) => {
      const sessions = ev.payload.sessions;
      let alwaysAsk = false;
      try {
        const v = await invoke<unknown>("settings_get", { key: "restore.always_ask" });
        alwaysAsk = v === true;
      } catch { /* setting missing → default false → silent attach */ }
      if (alwaysAsk) {
        setRestoreSessions(sessions);
        return;
      }
      // Silent auto-attach. If it fails, fall back to showing the modal so the
      // user can pick a recovery path manually.
      try {
        await invoke("boot_restore_attach");
        showToast("success", `Restaurada${sessions.length === 1 ? "" : "s"} ${sessions.length} sesión${sessions.length === 1 ? "" : "es"} previa${sessions.length === 1 ? "" : "s"}`);
      } catch (err) {
        console.warn("auto-attach failed, falling back to manual modal", err);
        setRestoreSessions(sessions);
      }
    }).then((u) => { off = u; });
    return () => { off?.(); };
  }, [showToast]);

  // BLOQUE D · F12 — clipboard auto-poll (1s) wrapped by smartpaste_offer's
  // gate; surfaces a single toast per never-seen content hash with a CTA to
  // open the full modal. Toast is intentionally low-key — no auto-open.
  usePolling(async () => {
    try {
      const text = await invoke<string | null>("clipboard_read");
      if (!text || text.length < 50) return;
      // Cheap hash: first/last 64 chars + length — collisions don't matter, we
      // just want "did the user copy something new?".
      const hash = `${text.length}|${text.slice(0, 64)}|${text.slice(-64)}`;
      if (recentPasteHashes.current.includes(hash)) return;
      const offered = await invoke<{ kind: string; bytes: number; lines: number; action_hint: string } | null>("smartpaste_offer", { text });
      if (!offered) {
        recentPasteHashes.current = [...recentPasteHashes.current.slice(-7), hash];
        return;
      }
      recentPasteHashes.current = [...recentPasteHashes.current.slice(-7), hash];
      showToast("info", `📋 Smart paste: ${offered.kind} (${offered.bytes}B). Click for ⌘⇧V.`);
    } catch {
      /* clipboard read can fail on permission denied — fail silent, no UX change */
    }
  }, { intervalMs: 1000, runOnMount: false });

  // BLOQUE C · F11-switcher — wire the CustomEvent "furx:set-cwd" emitted by
  // CommandPalette onPickProject so the focused pane actually picks up the new
  // cwd. Without this listener the cwd selected from the project switcher was
  // silently lost. We persist on PaneCfg.cwd (which is then re-sent to pty_spawn
  // on the next Terminal remount).
  useEffect(() => {
    const handler = (e: Event) => {
      const ce = e as CustomEvent<{ paneId: string; cwd: string }>;
      const { paneId, cwd } = ce.detail || {};
      if (!paneId || typeof cwd !== "string") return;
      setPanes((all) => all.map((p) => (p.id === paneId ? { ...p, cwd } : p)));
    };
    window.addEventListener("furx:set-cwd", handler);
    return () => window.removeEventListener("furx:set-cwd", handler);
  }, []);

  // C / F8 — merge_watcher event triggers a card refresh PLUS opens the
  // MergeReview modal so the user can see the diff stat without having to
  // dig into the card. The payload may not include repo_path/branch on older
  // backends; in that case we just refresh cards and skip the modal.
  useEffect(() => {
    let off: (() => void) | undefined;
    listen<{ card_id: string; worktree: string; repo_path?: string; branch?: string }>("furx:merge-suggest", (ev) => {
      refreshAll();
      const p = ev.payload;
      if (p.repo_path && p.branch) {
        setMergeReview({ repoPath: p.repo_path, branch: p.branch });
      }
    }).then((u) => { off = u; });
    return () => { off?.(); };
  }, [refreshAll]);

  // BLOQUE B · B/F2/F3: open-card event from CardItem button.
  // Codex must-fix consolidated:
  //   1. Reuse a pane that's genuinely Idle (zsh mode + empty buffer) instead
  //      of always spawning a new one.
  //   2. Persist cwd in PaneCfg so layout reload restores it.
  //   3. Inject bundle_path + card_id as the initial prompt via pty_write so
  //      the Claude pane actually sees the context (not just the cwd).
  useEffect(() => {
    const handler = async (e: Event) => {
      const ce = e as CustomEvent<{ paneSpec: { bundle_path: string | null; project_dir: string | null; suggested_mode: string; card_id?: string } }>;
      const spec = ce.detail.paneSpec;
      // 022 LOW — sin "claude-A" hardcodeado: si la card no sugiere modo, usá la PRIMERA
      // cuenta Claude configurada; si no hay ninguna, un modo neutro ("zsh").
      const firstClaude = claudeAccounts.find((a) => a.cli_kind === "claude");
      const fallbackMode = firstClaude ? `claude-${firstClaude.slug}` : "zsh";
      const desiredMode = (spec.suggested_mode || fallbackMode) as PaneMode;

      // 1. Find a genuinely idle pane (Codex must-fix #4: conservative idle).
      //    - zsh mode (won't yank a Claude session)
      //    - empty buffer tail (no recent PTY activity)
      //    - at least MIN_IDLE_AGE_MS since spawn (so we don't promote a pane
      //      whose prompt simply hasn't echoed yet — Codex audit MED #3).
      //    If none, create a fresh pane.
      const now = Date.now();
      let target: PaneCfg | null = null;
      for (const p of panes) {
        if (p.mode !== "zsh") continue;
        if (bufferOf(p.id).length > 0) continue;
        const bornAt = paneBornAt.current[p.id] ?? 0;
        if (bornAt === 0 || now - bornAt < MIN_IDLE_AGE_MS) continue;
        target = p; break;
      }

      // 022 LOW — el título sale del label DERIVADO (perfil → cuenta → slug legible),
      // nunca del mode crudo "claude-A". `derivePaneLabel` ya nunca emite "A/B".
      const cardTitle = `From card · ${derivePaneLabel(desiredMode, claudeAccounts, agents).label}`;
      let paneId: string;
      if (target) {
        paneId = target.id;
        // Promote to the suggested Claude mode + sticky cwd.
        setPanes((all) => all.map((x) =>
          x.id === paneId
            ? { ...x, mode: desiredMode, cwd: spec.project_dir ?? x.cwd, bundle_path: spec.bundle_path ?? undefined, title: cardTitle }
            : x
        ));
      } else {
        paneId = `card${Date.now()}-${Math.random().toString(36).slice(2,6)}`;
        paneBornAt.current[paneId] = Date.now();
        const fresh: PaneCfg = {
          id: paneId,
          mode: desiredMode,
          title: cardTitle,
          cwd: spec.project_dir ?? undefined,
          bundle_path: spec.bundle_path ?? undefined,
        };
        setPanes((p) => [...p, fresh]);
      }
      setFocusedPane(paneId);

      // 2. Pre-compile the per-pane bootstrap (used by claude-as wrapper).
      if (spec.project_dir) {
        await invoke("bootstrap_compile", { paneId, projectDir: spec.project_dir })
          .catch((err) => console.error("bootstrap_compile failed", err));
      }

      // 3. Inject the initial context prompt so the Claude pane actually sees
      //    the bundle. Codex audit MED #1: a fixed 1500ms timeout races with
      //    pty_spawn (history capture + listeners + remount on key change). Use
      //    a bounded retry loop: backoff 300/600/1200/2400/4800 ms (~9s total).
      if (spec.bundle_path || spec.project_dir || spec.card_id) {
        const parts: string[] = [];
        if (spec.card_id) parts.push(`Card: \`${spec.card_id}\``);
        if (spec.project_dir) parts.push(`Repo: \`${spec.project_dir}\``);
        if (spec.bundle_path) parts.push(`Context bundle: \`${spec.bundle_path}\` — leelo antes de actuar.`);
        if (parts.length > 0) {
          const prompt = `${parts.join("\n")}\n`;
          const correlationId = `open-card-${spec.card_id ?? "unknown"}`;
          const attempt = (delay: number, retriesLeft: number): void => {
            window.setTimeout(() => {
              invoke("pty_write", { paneId, data: prompt, correlationId, actionId: null })
                .catch((err) => {
                  if (retriesLeft <= 0) {
                    console.error("initial prompt write failed (gave up)", err);
                    return;
                  }
                  attempt(delay * 2, retriesLeft - 1);
                });
            }, delay);
          };
          attempt(300, 4); // 300 → 600 → 1200 → 2400 → 4800
        }
      }
    };
    window.addEventListener("furx:dispatch-open-card", handler);
    return () => window.removeEventListener("furx:dispatch-open-card", handler);
  }, [panes, bufferOf, claudeAccounts, agents]);

  // 022 US7 / FR-012 — entrada a la galería de agentes desde Ajustes (Settings dispara este evento
  // por el bus `furx:*` en vez de acoplarse a la chrome). Las otras entradas (command palette,
  // botón "agentes" de los panes) siguen abriendo la galería por su propio estado, sin cambios.
  useEffect(() => {
    const handler = () => setAgentGalleryOpen(true);
    window.addEventListener("furx:open-agents", handler);
    return () => window.removeEventListener("furx:open-agents", handler);
  }, []);

  // F18 — refresh audit list more frequently when the drawer is open.
  usePolling(async () => {
    const e = await invoke<AuditEvent[]>("list_events", { limit: 200 }).catch(() => []);
    setEvents(e);
  }, { intervalMs: 2000, enabled: auditDrawerOpen });

  // El remount del Terminal se hace via `key` en el componente Pane (mode-derived).
  // Mantenemos el pane.id estable así el layout persiste limpio en SQLite.
  // spec 004 F0/F3 — the "__data__" sentinel from the mode select flips the pane to a
  // (no-PTY) data viewer; any real mode flips it back to a terminal pane.
  const updateMode = useCallback((paneId: string, value: string) => {
    setPanes((p) => p.map((x) => {
      if (x.id !== paneId) return x;
      // 006 — "agent:<id>" asigna un agent profile (el backend maneja el runtime).
      if (value.startsWith("agent:")) {
        return { ...x, kind: "terminal" as const, agent_profile_id: value.slice("agent:".length) };
      }
      if (value === "__data__") {
        return { ...x, kind: "data" as const, title: x.kind === "data" ? x.title : "Data viewer" };
      }
      if (value === "__compare__") {
        return { ...x, kind: "compare" as const, title: x.kind === "compare" ? x.title : "Compare" };
      }
      if (value === "__web__") {
        return { ...x, kind: "web" as const, title: x.kind === "web" ? x.title : "Web" };
      }
      if (value === "__context__") {
        return { ...x, kind: "context" as const, title: x.kind === "context" ? x.title : "Context" };
      }
      // 022 US2 — "configurar cuenta": abre el wizard de cuentas en vez de setear un
      // modo fantasma "A/B". No muta el pane (el setup ocurre fuera).
      if (value === "__connect__") {
        setConnectOpen(true);
        return x;
      }
      // Cualquier modo legacy limpia el agente asignado (el `mode` vuelve a mandar).
      return { ...x, kind: "terminal" as const, mode: value as PaneMode, agent_profile_id: undefined };
    }));
  }, []);

  const updateWebUrl = useCallback((paneId: string, webUrl: string) => {
    setPanes((p) => p.map((x) => (x.id === paneId ? { ...x, web_url: webUrl } : x)));
  }, []);

  const updateContext = useCallback((paneId: string, repo: string, paths: string) => {
    setPanes((p) => p.map((x) => (x.id === paneId ? { ...x, context_repo: repo, context_paths: paths } : x)));
  }, []);

  // spec 004 follow-up — deliver another pane's (redacted) output into a data/compare view
  // pane instead of pty_write (which only works for terminal targets). Compare: fill the
  // empty side first.
  const deliverToView = useCallback((targetId: string, kind: "data" | "compare", text: string) => {
    setPanes((p) => p.map((x) => {
      if (x.id !== targetId) return x;
      if (kind === "data") return { ...x, data_content: text };
      return !x.compare_left ? { ...x, compare_left: text } : { ...x, compare_right: text };
    }));
  }, []);

  const updateDataContent = useCallback((paneId: string, content: string) => {
    setPanes((p) => p.map((x) => (x.id === paneId ? { ...x, data_content: content } : x)));
  }, []);

  const updateCompare = useCallback((paneId: string, left: string, right: string) => {
    setPanes((p) => p.map((x) => (x.id === paneId ? { ...x, compare_left: left, compare_right: right } : x)));
  }, []);

  const addPane = useCallback((modeArg: PaneMode = "zsh") => {
    // Guard defensivo: si un caller cablea `onClick={addPane}` (sin envolver), React pasa el evento
    // como 1er arg → `modeArg` sería un objeto sin `.startsWith` → crash. Aceptar solo strings.
    const mode: PaneMode = typeof modeArg === "string" ? modeArg : "zsh";
    // Codex MED: generate the id OUTSIDE setState so the updater stays pure;
    // setPanes' callback may be replayed by React under concurrent rendering.
    const newId = `p${Date.now()}-${Math.random().toString(36).slice(2, 6)}`;
    paneBornAt.current[newId] = Date.now();
    setPanes((p) => {
      if (p.length >= 8) return p;
      const titleFromMode = mode.startsWith("claude") ? "Claude"
        : mode.startsWith("codex") ? "Codex"
        : mode.startsWith("gemini") ? "Gemini"
        : mode.startsWith("aider") ? "Aider"
        : mode.startsWith("openai-api") ? "OpenAI"
        : mode === "zsh" ? "zsh" : "Pane";
      return [...p, { id: newId, mode, title: `${titleFromMode} ${p.length + 1}` }];
    });
    // Focus deferred so React commits the new pane first.
    requestAnimationFrame(() => setFocusedPane(newId));
  }, []);

  const removePane = useCallback((paneId: string) => {
    setPanes((p) => p.filter((x) => x.id !== paneId));
    forgetPane(paneId);
    delete paneBornAt.current[paneId];
  }, [forgetPane]);

  const cyclePaneMode = useCallback((paneId: string) => {
    // B9.1 — universal cycle: zsh → todos los CLI accounts agrupados por kind → legacy CLIs
    setPanes((p) => p.map((x) => {
      if (x.id !== paneId) return x;
      const cycleList: PaneMode[] = ["zsh"];
      for (const kind of ["claude", "codex", "gemini", "aider", "openai-api", "custom"]) {
        for (const a of claudeAccounts.filter((acc) => acc.cli_kind === kind)) {
          cycleList.push(`${kind}-${a.slug}` as PaneMode);
        }
      }
      // 022 LOW — sin placeholders fantasma "claude-A/B". Si no hay cuenta Claude
      // configurada, el ciclo NO inventa un modo: la entrada "configurar cuenta"
      // (sentinel __connect__) ya está en el dropdown de modos del pane.
      cycleList.push("codex" as PaneMode, "gemini" as PaneMode, "aider" as PaneMode, "grok" as PaneMode);
      const i = cycleList.indexOf(x.mode);
      const next = cycleList[(i + 1) % cycleList.length] ?? "zsh";
      return { ...x, mode: next };
    }));
  }, [claudeAccounts]);

  // BLOQUE A · G — single snapshot trigger reused by ⌘⇧S and the actions palette.
  // Guards against piled-up requests (Codex qa-edge-case) and surfaces both
  // success and failure via toast (SRE/UX must-fix).
  const runManualSnapshot = useCallback(() => {
    if (snapshotInFlight.current) return;
    snapshotInFlight.current = true;
    invoke<unknown>("snapshot_take", { kind: "manual" })
      .then(() => showToast("success", "Snapshot saved"))
      .catch((err) => {
        console.error("snapshot_take failed", err);
        const msg = err instanceof Error ? err.message : String(err);
        showToast("error", `Snapshot failed: ${msg}`);
      })
      .finally(() => { snapshotInFlight.current = false; });
  }, [showToast]);

  // Codex MED — centralized modal manager: opening any modal closes the rest.
  // ⌘K audit (Codex MED-2 2026-05-26): extend manager to standup/pr/disagree to avoid
  // modal stacking (open Broadcast → ⌘K → run Standup → both visible).
  type ModalKind = "palette-search" | "palette-project" | "palette-actions" | "broadcast" | "smartpaste" | "voice" | "council" | "standup" | "pr" | "disagree";
  const openModal = useCallback((kind: ModalKind | null) => {
    setPaletteMode(
      kind === "palette-search" ? "search"
        : kind === "palette-project" ? "project"
        : kind === "palette-actions" ? "actions"
        : null,
    );
    setBroadcastOpen(kind === "broadcast");
    setSmartPasteOpen(kind === "smartpaste");
    setVoiceOpen(kind === "voice");
    setCouncilOpen(kind === "council");
    setStandupOpen(kind === "standup");
    setPrModalOpen(kind === "pr");
    setDisagreeOpen(kind === "disagree");
  }, []);

  // 047 FR-001 — acciones contextuales del ⌘K (CommandPalette015). Combina:
  //  (a) acciones GLOBALES movidas del top bar P0 (PR Description, Disagree) + atajos potentes
  //      (Broadcast, Council, Standup, Agentes), y
  //  (b) acciones de LA VISTA activa (la vista ofrece sus acciones arriba).
  // Todas son aperturas/navegación HUMANAS explícitas (abren un modal / agregan un pane) — nunca
  // auto-disparan trabajo. El palette las ejecuta vía `run` y se cierra.
  // 047 FR-007 — "Detener agentes": pausa (SIGSTOP) todas las tareas corriendo, detrás de una
  // CONFIRMACIÓN humana explícita (nunca auto). Reanudar es por-tarea (orchestration_resume_task).
  const stopAllAgents = useCallback(async () => {
    const ok = typeof window !== "undefined" && typeof window.confirm === "function"
      ? window.confirm("¿Detener (pausar) todos los agentes que están corriendo? Quedan congelados; los reanudás cuando quieras.")
      : true;
    if (!ok) return;
    try {
      const n = await invoke<number>("stop_all_agents", {});
      showToast(n > 0 ? "success" : "info", n > 0 ? `${n} agente(s) pausado(s)` : "No había agentes corriendo", 3500);
    } catch (e) {
      showToast("error", `No se pudieron detener todos: ${typeof e === "string" ? e : (e as Error)?.message ?? "error"}`, 5000);
    }
  }, [showToast]);

  const contextActions = useMemo<ContextAction[]>(() => {
    const acts: ContextAction[] = [];
    // (b) por vista activa — primero, para que queden arriba con el search vacío.
    if (view === "panes") {
      acts.push({ id: "ctx.panes.add", label: "Agregar panel", description: "Nuevo panel (⌘N)", group: "Esta vista", run: () => addPane() });
      acts.push({ id: "ctx.panes.broadcast", label: "Difundir a todos los paneles…", description: "Enviar un mensaje a cada panel (⌘B)", group: "Esta vista", run: () => openModal("broadcast") });
    }
    // 047 FR-007 — "Detener agentes" disponible globalmente desde el palette (con confirmación).
    acts.push({ id: "ctx.global.stop", label: "Detener agentes (pausar todo)…", description: "Pausa (SIGSTOP) las tareas corriendo — con confirmación", group: "Acciones", run: () => { void stopAllAgents(); } });
    if (view === "incidents") {
      acts.push({ id: "ctx.incidents.audit", label: "Abrir auditoría en vivo", description: "Drawer de eventos de audit", group: "Esta vista", run: () => setAuditDrawerOpen(true) });
    }
    // (a) acciones globales movidas del top bar (P0 las sacó del render del top bar).
    // 047 FR-001 (audit-3 HIGH) — "Describir PR…" es una sola acción global; en la vista github
    // la promovemos al grupo "Esta vista" (no la duplicamos). Sin doble fila idéntica.
    const prGroup = view === "github" ? "Esta vista" : "Acciones";
    acts.push({ id: "ctx.global.pr", label: "Describir PR…", description: "Generar la descripción de un Pull Request", group: prGroup, run: () => openModal("pr") });
    acts.push({ id: "ctx.global.disagree", label: "Registrar desacuerdo…", description: "Abrir el panel de desacuerdo entre agentes", group: "Acciones", run: () => openModal("disagree") });
    acts.push({ id: "ctx.global.council", label: "Convocar Council…", description: "Comparar respuestas de varios agentes (⌘J)", group: "Acciones", run: () => openModal("council") });
    acts.push({ id: "ctx.global.standup", label: "Standup…", description: "Resumen de la sesión", group: "Acciones", run: () => openModal("standup") });
    acts.push({ id: "ctx.global.agents", label: "Galería de agentes…", description: "Ver y configurar agentes", group: "Acciones", run: () => setAgentGalleryOpen(true) });
    // Dedup por id (la vista `github` y la global de PR comparten intención): el primero gana.
    const seen = new Set<string>();
    return acts.filter((a) => (seen.has(a.id) ? false : (seen.add(a.id), true)));
  }, [view, addPane, openModal, stopAllAgents]);

  // 047 FR-003 — agentes (agent_profile_id) que están corriendo en algún pane. La AgentGallery
  // les pinta un borde teal. Set para lookup O(1) por id.
  const activeAgentIds = useMemo(
    () => new Set(panes.map((p) => p.agent_profile_id).filter((id): id is string => !!id)),
    [panes],
  );

  // Keyboard shortcuts (⌘1..⌘8 focus · ⌘N add · ⌘W close · ⌘⇧M cycle mode · ⌘⇧V voice).
  useEffect(() => {
    const handler = (e: KeyboardEvent) => {
      // ⌘/ or bare "?" → shortcut sheet (with input/textarea/contenteditable gate).
      // Council MED: KeyboardEvent.key respects user locale layout.
      const target = e.target as HTMLElement | null;
      const inEditable = !!target && (
        target.tagName === "INPUT" || target.tagName === "TEXTAREA" ||
        (target as HTMLElement).isContentEditable
      );
      // 047 FR-005 — Esc sale de Focus Mode. NO le robamos el Esc al terminal: si el foco de teclado
      // está en un editable (la textarea de xterm, un input), Esc va al PTY/campo. Sólo sale del modo
      // cuando el foco está en el "chrome" (no editable). El ⌘⇧F (toggle) y el botón "Salir" cubren
      // el caso de estar tipeando en el pane.
      if (focusModeOn && e.key === "Escape" && !inEditable) {
        e.preventDefault();
        setFocusModeOn(false);
        return;
      }
      if ((e.metaKey && e.key === "/") || (!e.metaKey && !e.ctrlKey && e.key === "?" && !inEditable)) {
        e.preventDefault();
        const wasOpen = shortcutSheetOpen;
        if (!wasOpen && (paletteMode !== null || broadcastOpen || smartPasteOpen
          || voiceOpen || councilOpen || standupOpen || prModalOpen || disagreeOpen
          || restoreSessions !== null || pendingSuggestion !== null
          || connectOpen || needsWizard)) return;
        setShortcutSheetOpen(!wasOpen);
        return;
      }
      if (!e.metaKey) return;
      // Codex MED-1: gate pane mutators (⌘N/⌘W/⌘⇧M/⌘1-8) when any modal/palette is open.
      // Toggle-style shortcuts (⌘K/⌘P/⌘B/⌘J/⌘⇧V/⌘⇧K/⌘⇧S) keep working — they manage modals.
      // Codex HIGH v1: include every dialog state so global shortcuts can't mount
      // a hidden surface behind a visible modal (e.g. ⌘K palette behind ProGate).
      const anyModalOpen = paletteMode !== null || broadcastOpen || smartPasteOpen
        || voiceOpen || councilOpen || standupOpen || prModalOpen || disagreeOpen
        || shortcutSheetOpen || cmd015Open
        || restoreSessions !== null || pendingSuggestion !== null
        || connectOpen || needsWizard;
      const idx = parseInt(e.key, 10);
      if (idx >= 1 && idx <= 8 && idx <= panes.length) {
        if (anyModalOpen) return;
        e.preventDefault();
        const p = panes[idx - 1];
        if (p) setFocusedPane(p.id);
        return;
      }
      if (e.shiftKey && e.key.toLowerCase() === "m" && focusedPane) {
        if (anyModalOpen) return;
        e.preventDefault(); cyclePaneMode(focusedPane); return;
      }
      // Codex HIGH v2: gate OPENING (not closing) when another modal is already up.
      if (e.shiftKey && e.key.toLowerCase() === "v") {
        e.preventDefault();
        if (!voiceOpen && anyModalOpen) return;
        openModal(voiceOpen ? null : "voice");
        return;
      }
      if (e.key.toLowerCase() === "p" && !e.shiftKey) {
        e.preventDefault();
        const wasOpen = paletteMode === "search";
        if (!wasOpen && anyModalOpen) return;
        openModal(wasOpen ? null : "palette-search");
        return;
      }
      if (e.key.toLowerCase() === "k" && !e.shiftKey) {
        // ⌘K — Command Palette universal (US2, spec 015). Indexa el command
        // registry tipado (US1) con fuzzy search + filtros + confirmación de
        // destructivos. Reemplaza la palette de acciones legacy en ⌘K; la
        // migración de nav completa es US10.
        e.preventDefault();
        if (!cmd015Open && anyModalOpen) return;
        if (!cmd015Open) openModal(null); // cerrar cualquier palette legacy abierta
        setCmd015Open((v) => !v);
        return;
      }
      if (e.shiftKey && e.key.toLowerCase() === "k") {
        // ⌘⇧K — legacy project switcher (muscle memory de v0.2-byok).
        e.preventDefault();
        const wasOpen = paletteMode === "project";
        if (!wasOpen && anyModalOpen) return;
        openModal(wasOpen ? null : "palette-project");
        return;
      }
      if (e.key.toLowerCase() === "b" && !e.shiftKey) {
        e.preventDefault();
        if (!broadcastOpen && anyModalOpen) return;
        openModal(broadcastOpen ? null : "broadcast");
        return;
      }
      if (e.shiftKey && e.key.toLowerCase() === "s") {
        e.preventDefault();
        if (e.repeat) return; // ignore held-down key
        runManualSnapshot();
        return;
      }
      if (e.shiftKey && e.key.toLowerCase() === "f") {
        // 047 FR-005 — ⌘⇧F: Focus Mode. Sólo tiene sentido con un pane focado en la vista de paneles.
        // Toggle (re-presionar sale). NO se abre con un modal arriba (sería confuso). Esc también sale.
        e.preventDefault();
        if (e.repeat) return;
        if (focusModeOn) { setFocusModeOn(false); return; }
        if (anyModalOpen || view !== "panes" || !focusedPane) return;
        setFocusModeOn(true);
        return;
      }
      if (e.shiftKey && (e.key === "." || e.code === "Period")) {
        // 031 F1b — ⌘⇧. "callar": silencia los avisos de audio de la cola de atención. Global y
        // safe (no toca foco ni datos), funciona aun con un modal abierto. Idempotente.
        e.preventDefault();
        if (e.repeat) return;
        invoke("callar", {}).then(() => showToast("info", "Avisos de audio silenciados", 3000)).catch(() => {});
        return;
      }
      if (e.shiftKey && (e.key.toLowerCase() === "n")) {
        // 032 U1 — ⌘⇧N: enfocar VISUALMENTE el siguiente pane que reclama atención (cicla por
        // prioridad). NO mueve el foco del mic (sólo la voz lo concede). No-op si la cola está vacía.
        if (anyModalOpen) return;
        e.preventDefault();
        if (e.repeat) return;
        invoke<string | null>("attention_next_pane", { current: focusedPane ?? null })
          .then((next) => { if (next) setFocusedPane(next); })
          .catch(() => {});
        return;
      }
      if (e.key.toLowerCase() === "j" && !e.shiftKey) {
        e.preventDefault();
        if (!councilOpen && anyModalOpen) return;
        const now = Date.now();
        if (!councilOpen && now - councilCooldown.current < 5000) return;
        councilCooldown.current = now;
        // Council Mode es FREE (OPEN-CORE.md "free forever"; BLOQUE 1 lo gateaba como Pro —
        // legacy de la monetización vieja, contradecía el canon publicado). Sin gate.
        openModal(councilOpen ? null : "council");
        return;
      }
      if (e.key.toLowerCase() === "n" && !e.shiftKey) {
        if (anyModalOpen) return;
        e.preventDefault(); addPane(); return;
      }
      if (e.key.toLowerCase() === "w" && focusedPane) {
        if (anyModalOpen) return;
        e.preventDefault(); removePane(focusedPane); setFocusedPane(null); return;
      }
    };
    window.addEventListener("keydown", handler);
    return () => window.removeEventListener("keydown", handler);
  }, [panes, focusedPane, cyclePaneMode, addPane, removePane, openModal, paletteMode, broadcastOpen, smartPasteOpen, voiceOpen, councilOpen, standupOpen, prModalOpen, disagreeOpen, shortcutSheetOpen, cmd015Open, restoreSessions, pendingSuggestion, connectOpen, needsWizard, proActive, focusModeOn, view]);

  // 047 FR-005 — auto-salir de Focus Mode si se rompe su precondición (se perdió el pane focado, o se
  // navegó fuera de la vista de paneles). Evita quedar en un modo "vacío" sin pane que mostrar.
  useEffect(() => {
    if (focusModeOn && (view !== "panes" || !focusedPane)) setFocusModeOn(false);
  }, [focusModeOn, view, focusedPane]);

  // 047 FR-005 (audit-3 codex) — al entrar/salir de Focus Mode el contenedor del pane focado cambia
  // de tamaño (los demás pasan a display:none). El xterm de CADA Terminal ya re-fitea vía su propio
  // ResizeObserver sobre su container (Terminal.tsx) — la transición display:none↔visible y el cambio
  // de la celda de grid disparan ese observer, así que el terminal NO queda con dimensiones viejas.
  // Emitimos además un `resize` global como belt-and-suspenders (algunos componentes lo escuchan)
  // tras un frame, sin acoplarnos al ciclo de fit de xterm.
  useEffect(() => {
    const id = requestAnimationFrame(() => window.dispatchEvent(new Event("resize")));
    return () => cancelAnimationFrame(id);
  }, [focusModeOn]);

  // 044 FR-003 — guard anti doble-respuesta de las acciones de card (las decisiones son HUMANAS; el
  // guard sólo evita la carrera de UI). `decidingCardId` deshabilita los botones de la card en vuelo;
  // `cardErrors` muestra el error inline (la card NO desaparece); a los 15s sin respuesta re-habilita.
  const decideGuard = useDecideGuard();
  const decide = (cardId: string, decision: string, snoozeUntil?: string) => {
    // El prompt de nota es síncrono y va ANTES del guard (no debe contar contra el timeout/seq).
    const note = (decision === "rejected" || decision === "needs-changes")
      ? prompt(`Note for "${decision}":`) ?? undefined : undefined;
    // 050 FR-004 — idempotency key estable POR DECISIÓN (defensa extra sobre el seqRef de la Ola 3,
    // para el caso multi-instancia futuro: dos ventanas/instancias que replayen la MISMA decisión).
    // El seqRef ya cubre la doble-respuesta intra-ventana; esto cubre el cross-instancia. La key se
    // genera UNA vez acá (no en cada retry del guard) → un retry usa la MISMA key, idempotente.
    const idempotencyKey = makeIdempotencyKey();
    // El `action` hace SÓLO el invoke (sin efectos de vista). `refreshAll` va como `onApplied` →
    // corre SÓLO si la resolución sigue vigente (audit-3 fix: una respuesta tardía post-timeout NO
    // refresca/remueve la card).
    decideGuard.run(
      cardId,
      () => invoke("decide_card", { cardId, decision, note, snoozeUntil: snoozeUntil ?? null, idempotencyKey }).then(() => undefined),
      () => { void refreshAll(); },
    );
  };

  // 022 P1 · US6 — "ir al origen": navega a la vista canónica de la fuente de la card (monitor→
  // monitores filtrado a caídos; merge/worktree→paneles). Si la fuente no tiene vista (null), el
  // caller abre el slide-over de detalle en su lugar (lo decide la UI con `sourceTarget`).
  const goToCardSource = (card: Card) => {
    const target = sourceTarget(card);
    if (!target.view) return; // sin vista canónica → la UI abre el detalle.
    if (target.drilldown === "monitors-down") {
      navigate(target.view, { view: "monitors", status: "down" });
    } else {
      navigate(target.view);
    }
  };

  // 047 FR-004 — trazabilidad card→audit: abre el drawer de audit con el evento de ESTA card
  // resaltado (su grupo de sesión se auto-expande). Refresca la lista al abrir (el poll del drawer
  // ya estaba ligado a `auditDrawerOpen`). Acción de navegación humana (no muta nada).
  const openAuditForCard = useCallback((cardId: string) => {
    setAuditHighlightCardId(cardId);
    setAuditDrawerOpen(true);
  }, []);



  // 022 P0b/P1 — reloj barato que avanza cada 5s. Alimenta la freshness de los stats Y el cálculo
  // del inbox (para que una card snoozeada reaparezca al expirar sin esperar un refetch del backend).
  const [statsNow, setStatsNow] = useState<number>(Date.now());
  useEffect(() => {
    const id = window.setInterval(() => setStatsNow(Date.now()), 5000);
    return () => window.clearInterval(id);
  }, []);
  // 022 P1 · US6 — el rail y el stat de incidentes muestran lo ACCIONABLE del inbox (open, visible,
  // sin snooze/dismiss). Una card snoozeada sale del rail; reaparece (auto-unsnooze) cuando expira o
  // su fuente registra nueva actividad. `nowIso` en UTC formato SQLite (comparable con el backend).
  const nowIso = useMemo(
    () => new Date(statsNow).toISOString().slice(0, 19).replace("T", " "),
    [statsNow],
  );
  const openCards = useMemo(() => inboxCards(cards, nowIso), [cards, nowIso]);
  const upMons = monitors.filter((m) => m.last?.up).length;
  const [railCollapsed, setRailCollapsed] = useState(false);

  // 022 P0b · REFORMA 3 — navegar a la vista que origina un stat, aplicando su filtro
  // de drill-down (incidentes→abiertos, monitors→down). Reusa el `setView` existente.
  const goToStat = (stat: ActionableStat) => {
    // Atómico: setea vista + filtro de drill-down juntos (one-shot). Es el ÚNICO call-site que
    // pasa un filtro no-null; toda otra nav usa `navigate(view)` que lo deja en `null`.
    navigate(stat.destView, statFilterToViewFilter(stat));
  };
  // 022 P0c · US5/FR-008 — las etiquetas/aria de los stats salen del catálogo i18n vía `t`.
  // Wrapper laxo: el `t` real exige params tipados por key; acá el translator es genérico (la
  // paridad de placeholders ya la valida `tsc -b` sobre el catálogo).
  const sidebarStats = buildSidebarStats(
    {
      openIncidents: openCards.length,
      panes: panes.length,
      monitorsUp: upMons,
      monitorsTotal: monitors.length,
    },
    (key, params) => t(key, params as never),
  );
  // Freshness barato del último poll de cards (no obligatorio, informativo). Reusa `statsNow` (arriba).
  const statsFreshness = freshnessLabel(lastRefreshAt, statsNow, (key, params) => t(key, params as never));

  // 022 P0b · REFORMA 4 — los shortcuts del sidebar DERIVAN del registry real
  // (`buildActions()`), misma fuente que el ShortcutSheet. Cero literales.
  const sidebarShortcuts = featuredSidebarShortcuts(
    buildActions({
      panes, focusedPane, proActive,
      addPane, removePane, cyclePaneMode, setFocusedPane,
      setView: (v) => navigate(v as View),
      openBroadcast: () => openModal("broadcast"),
      openVoice: () => openModal("voice"),
      openSmartPaste: () => openModal("smartpaste"),
      openStandup: () => openModal("standup"),
      openPr: () => openModal("pr"),
      openDisagree: () => openModal("disagree"),
      openCouncil: () => openModal("council"),
      setAuditDrawerOpen,
      snapshotManual: runManualSnapshot,
      readAloud,
      stopReadAloud,
      ttsAvailable,
      toggleAutoRead: () => { if (focusedPane) toggleAutoRead(focusedPane); },
      autoReadOn: focusedPane ? autoReadPanes.has(focusedPane) : false,
    }),
  );

  // 042 FR-001 — mientras carga settings, NO renderizamos la shell (evita flash de contenido sin
  // contexto). El spinner aparece sólo si el `loading` dura más que el delay anti-flash.
  if (settingsState === "loading") {
    return (
      <div className="shell-boot" role="status" aria-live="polite" aria-busy="true">
        {showBootSpinner && (
          <div className="boot-spinner">
            <span className="spinner" aria-hidden="true" />
            <span className="muted">Cargando Furx…</span>
          </div>
        )}
      </div>
    );
  }

  return (
    <div
      className="shell"
      data-view={view}
      data-rail-collapsed={railCollapsed ? "true" : "false"}
      /* 047 FR-005 — Focus Mode: oculta sidebar/top bar y deja sólo el pane focado a full ventana. */
      data-focus-mode={focusModeOn && view === "panes" && focusedPane ? "true" : "false"}
    >
      {focusModeOn && view === "panes" && focusedPane && (
        <button
          type="button"
          className="focus-mode-exit"
          onClick={() => setFocusModeOn(false)}
          title="Salir de Focus Mode (Esc o ⌘⇧F)"
          aria-label="Salir de Focus Mode"
        >
          ⤢ Salir · Esc
        </button>
      )}
      <aside className="sidebar" data-tour="sidebar">
        <div className="brand">
          <span className="hex" />
          <div>
            <div className="brand-name">Furx</div>
            <div className="brand-sub">v{version}</div>
          </div>
          {/* 055 wedge — diferencial móvil siempre a la vista: "continuá en el teléfono". El punto
              verde indica que el bridge está corriendo (emparejable); el click abre Ajustes → Móvil. */}
          <button
            type="button"
            className="brand-mobile"
            onClick={() => navigate("settings")}
            aria-label={mobileRunning ? "Bridge móvil activo — abrir Ajustes → Móvil" : "Conectar el teléfono — abrir Ajustes → Móvil"}
            title={mobileRunning ? "Móvil: continuá esta sesión en el teléfono (Ajustes → Móvil)" : "Conectá tu teléfono (Ajustes → Móvil)"}
          >
            <Smartphone size={15} strokeWidth={1.75} aria-hidden="true" />
            <span className={`brand-mobile-dot ${mobileRunning ? "on" : ""}`} aria-hidden="true" />
          </button>
        </div>
        <SidebarGroups<View>
          activeView={view}
          // nav normal por sidebar → one-shot: re-entrar a Incidents/Monitors limpia el filtro.
          onSelect={(v) => navigate(v)}
          // 015 T020 — la taxonomía (6 dominios) es el SSOT testeable en lib/navGroups; acá sólo
          // inyectamos los badges dinámicos por vista. `grouped` = flag (OFF → lista plana).
          // 022 P0c · US5 — los labels (grupo + ítem) salen del catálogo i18n (sentence-case),
          // el `label` literal de NAV_GROUPS queda como fallback de la serialización móvil.
          groups={NAV_GROUPS.map((g) => ({
            ...g,
            label: t(navGroupLabelKey(g.id)),
            items: g.items.map((it) => {
              // 058 (ultrareview fix): se quitaron las ramas `incidents`/`audit` — tras 055 NO están en
              // NAV_GROUPS (este .map sólo itera la espina), así que eran código MUERTO (nunca matcheaban).
              const badge =
                it.view === "panes" ? String(panes.length)
                  : it.view === "memory" ? (memoryCount && memoryCount > 0 ? String(memoryCount) : undefined)
                    : it.view === "activity" ? (monitors.length > 0 ? `${upMons}/${monitors.length}` : undefined)
                      : undefined;
              const base = { ...it, label: t(navItemLabelKey(it.view)) };
              return badge ? { ...base, badge } : base;
            }),
          })) satisfies SidebarGroupSpec<View>[]}
          grouped={navGrouped}
        />
        {/* 022 P0b · REFORMA 3 — cada stat es una PUERTA a la vista que lo origina
            (drill-down + filtro). Cero literal de estado ("Schema v3" eliminado). */}
        <div className="stats" role="group" aria-label={t("chrome.stats.group")}>
          {sidebarStats.map((s) => (
            <button
              key={s.id}
              type="button"
              className="row stat-action"
              onClick={() => goToStat(s)}
              aria-label={s.ariaLabel}
              title={`${s.ariaLabel}${statsFreshness ? ` · ${statsFreshness}` : ""}`}
            >
              <span>{s.label}</span>
              <strong>{s.value}</strong>
            </button>
          ))}
          {statsFreshness && (
            <div className="row stat-freshness muted" aria-hidden="true">
              <span>{t("chrome.stats.updated")}</span><span>{statsFreshness}</span>
            </div>
          )}
        </div>
        {/* 022 P0b · REFORMA 4 — shortcuts DERIVADOS del registry real (buildActions),
            misma fuente que el ShortcutSheet. Cero literales desincronizados. */}
        <div className="shortcuts">
          <div className="muted">{t("chrome.shortcuts.heading")}</div>
          {sidebarShortcuts.map((s) => (
            <div key={s.id}><kbd>{s.shortcut}</kbd> {s.label.toLowerCase()}</div>
          ))}
          <button
            type="button"
            className="shortcuts-all ghost"
            onClick={() => setShortcutSheetOpen(true)}
            aria-label={t("chrome.shortcuts.allAria")}
          >
            <kbd>⌘/</kbd> {t("chrome.shortcuts.all").toLowerCase()}
          </button>
        </div>
        {/* 059 — se removieron del fondo del sidebar: el botón Seed-demo-cards (dev) (herramienta de
            dev) y el toggle "Agrupado/Plano" (rollback de nav; el flag `groupedNav` lo cubre sin botón). */}
      </aside>

      <main className="main">
        {/* 042 FR-003 — banner discreto NO bloqueante: el endpoint AIE configurado/default no
            responde. Link a Ajustes→Servicios. Dismissible por sesión (no re-aparece tras cerrarlo). */}
        <InfraBanner
          visible={infraUnreachable && !infraBannerDismissed && !needsWizard}
          onOpenSettings={() => navigateInternal.navigate("furx://settings/endpoints")}
          onDismiss={() => setInfraBannerDismissed(true)}
        />
        <UpdateBanner
          mode="auto"
          confirmRestart={() => {
            const activePty = panes.length;
            const msg = activePty > 0
              ? `Install update and restart Furx now? ${activePty} active pane${activePty === 1 ? "" : "s"} will be terminated (tmux sessions persist if installed).`
              : "Install update and restart Furx now?";
            return typeof window !== "undefined" && typeof window.confirm === "function"
              ? window.confirm(msg)
              : true;
          }}
        />
        <TopBar
          usage={usage} usageStaleAt={usageStaleAt}
          aieState={aieState} aieStaleAt={aieStaleAt}
          auditDrawerOpen={auditDrawerOpen}
          onToggleAudit={() => setAuditDrawerOpen((v) => !v)}
          onOpenSmartPaste={() => openModal("smartpaste")}
          onOpenStandup={() => setStandupOpen(true)}
          onOpenPr={() => setPrModalOpen(true)}
          onOpenDisagree={() => setDisagreeOpen(true)}
          onOpenHelp={helpEnabled ? () => openHelp(view, "topbar") : undefined}
          // 022 P0b · REFORMA 3 — tokens → detalle de uso/consumo (vista AIE providers/latency).
          onOpenUsage={() => navigate("latency")}
        />
        {/* 015 T021 — error boundary PER-PANEL: un crash en UNA vista muestra un fallback local
            (key={view} → se resetea al cambiar de vista) sin tumbar TopBar/sidebar ni el backend. */}
        <ErrorBoundary scope="panel" key={view} name={view}>
        {view === "panes" && panes.length === 0 && (
          <EmptyShellState
            onOpenPane={addPane}
            onOpenWizard={() => setNeedsWizard(true)}
            hasClaudeAccount={claudeAccounts.some((a) => a.cli_kind === "claude")}
            tmuxAvailable={tmuxAvailable}
            claudeMode={(() => {
              // 022 LOW — modo Claude REAL de la primera cuenta, nunca "claude-A".
              const c = claudeAccounts.find((a) => a.cli_kind === "claude");
              return c ? (`claude-${c.slug}` as PaneMode) : null;
            })()}
            claudeLabel={(() => {
              const c = claudeAccounts.find((a) => a.cli_kind === "claude");
              return c ? derivePaneLabel(`claude-${c.slug}`, claudeAccounts, agents).label : null;
            })()}
          />
        )}
        {/* 018 Fase 2 US1 — flag ON: Workspace flexible (dockview desde LayoutConfigV1). Cada
            Leaf monta su Pane REAL por panel_id (reusa el proceso vivo, no respawnea). flag
            OFF: grilla 2×2 legacy (ROLLBACK). */}
        {view === "panes" && panes.length > 0 && newWorkspace && (
          <WorkspaceView
            renderLeaf={({ panelId, leaseWindowLabel, leaseMountInstanceId }) => {
              const p = panes.find((x) => x.id === panelId);
              if (!p) return null;
              return (
                <Pane
                  pane={p}
                  focused={p.id === focusedPane}
                  recording={p.id === recordingPane}
                  voiceState={p.id === recordingPane ? voiceState : "idle"}
                  suggestion={paneSuggestions[p.id] ?? null}
                  claudeAccounts={claudeAccounts}
                  agents={agents}
                  onFocus={() => setFocusedPane(p.id)}
                  onModeChange={updateMode}
                  onDataChange={updateDataContent}
                  onCompareChange={updateCompare}
                  onWebUrlChange={updateWebUrl}
                  onContextChange={updateContext}
                  onRemove={() => removePane(p.id)}
                  onOutput={(data) => captureOutput(p.id, data)}
                  onSuggestionClick={onSuggestionClick}
                  onSendLast={() => setInterpaneSource(p)}
                  leaseWindowLabel={leaseWindowLabel}
                  leaseMountInstanceId={leaseMountInstanceId}
                  bornAt={paneBornAt.current[p.id] ?? 0}
                />
              );
            }}
          />
        )}
        {view === "panes" && panes.length > 0 && !newWorkspace && (
          <PanesView
            panes={panes} gridCols={gridCols} gridRows={gridRows}
            focusedPane={focusedPane}
            recordingPane={recordingPane}
            voiceState={voiceState}
            claudeAccounts={claudeAccounts}
            agents={agents}
            onFocus={setFocusedPane}
            onModeChange={updateMode}
            onDataChange={updateDataContent}
            onCompareChange={updateCompare}
            onWebUrlChange={updateWebUrl}
            onContextChange={updateContext}
            onAdd={addPane} onRemove={removePane}
            onGridCols={setGridCols} onGridRows={setGridRows}
            onOutput={captureOutput}
            paneSuggestions={paneSuggestions}
            onSuggestionClick={onSuggestionClick}
            onSendLast={setInterpaneSource}
            onOpenAgents={() => setAgentGalleryOpen(true)}
            onOpenOrch={() => setOrchOpen(true)}
            bornAtOf={(id) => paneBornAt.current[id] ?? 0}
            onStopAll={stopAllAgents}
          />
        )}
        {view === "incidents" && (
          <CardsView
            cards={cards}
            onDecide={decide}
            onGoToSource={goToCardSource}
            onOpenAudit={openAuditForCard}
            // 022 P0b — drill-down desde el stat: pre-filtrar a abiertos/accionables.
            initialOpenOnly={viewFilter?.view === "incidents"}
            onClearFilter={() => setViewFilter(null)}
            // 044 FR-002 — skeleton/error/retry del inbox.
            loading={cardsLoading}
            error={cardsError}
            onRetry={() => { setCardsLoading(true); setCardsError(null); refreshAll(); }}
            // 044 FR-003 — guard anti doble-respuesta (deshabilita botón en vuelo + error inline por card).
            // Pasamos el predicado `isDeciding` (no un id único) para soportar varias cards en vuelo a la vez.
            isDeciding={decideGuard.isDeciding}
            cardErrors={decideGuard.cardErrors}
          />
        )}
        {view === "monitors" && (
          <MonitorsView
            monitors={monitors}
            // 022 P0b — drill-down desde el stat: pre-filtrar a los caídos.
            initialDownOnly={viewFilter?.view === "monitors"}
            onClearFilter={() => setViewFilter(null)}
            // 045 FR-001 — refrescar la lista tras agregar/quitar un target.
            onChanged={refreshAll}
          />
        )}
        {view === "audit" && <AuditView events={events} />}
        {view === "saas" && <SaasView />}
        {view === "health" && <McpHealthView />}
        {view === "heatmap" && <HeatmapView />}
        {view === "grafana" && <GrafanaView />}
        {/* 047 FR-006 — Extensiones unificada. La entrada del sidebar es `extensions`;
            los deep-links viejos `furx://plugins` / `furx://tools` siguen vivos y montan
            la MISMA vista con la tab pre-seleccionada (cero regresión de links). */}
        {view === "extensions" && <ExtensionsView />}
        {view === "plugins" && <ExtensionsView initialTab="plugins" />}
        {view === "ssh" && (
          <SshView
            panes={panes}
            focusedPane={focusedPane}
            /* BLOQUE G · F22 — spawn a fresh zsh pane and queue the ssh
               command via the same retry-write pattern used by Card→Claude. */
            onOpenSshPane={(h) => {
              const newId = `ssh-${h.name}-${Date.now()}`.replace(/[^A-Za-z0-9_.\-@]/g, "_");
              paneBornAt.current[newId] = Date.now();
              setPanes((p) => [...p, { id: newId, mode: "zsh", title: `ssh · ${h.name}` }]);
              setFocusedPane(newId);
              const attempt = (delay: number, retriesLeft: number) => {
                window.setTimeout(() => {
                  invoke("pty_write", {
                    paneId: newId,
                    data: `ssh ${h.name}\n`,
                    correlationId: `ssh-open-${newId}`,
                    actionId: null,
                  }).catch((err) => {
                    if (retriesLeft <= 0) console.error("ssh write failed", err);
                    else attempt(delay * 2, retriesLeft - 1);
                  });
                }, delay);
              };
              attempt(300, 4);
            }}
          />
        )}
        {view === "vpn" && <VpnView />}
        {view === "latency" && <LatencyView />}
        {view === "reliability" && <ReliabilityView />}
        {view === "activity" && <ActivityView onNavigate={navigate} />}
        {view === "savings" && <SavingsMeter />}
        {view === "search" && <SearchView />}
        {view === "eval" && <EvalView />}
        {view === "queue" && <QueueView />}
        {view === "router" && <RouterView />}
        {view === "replay" && <ReplayView />}
        {view === "tools" && <ExtensionsView initialTab="skills" />}
        {view === "memory" && <MemoryView />}
        {view === "settings" && <SettingsView />}
        {view === "crashlog" && <CrashLogView />}
        {view === "github" && <GithubView />}
        {/* 053 — vistas cableadas para huérfanos backend */}
        {view === "policy" && <PolicyView />}
        {view === "presets" && <PresetView />}
        </ErrorBoundary>
      </main>

      <CardsRail openCount={openCards.length} onCollapsedChange={setRailCollapsed}>
        {openCards.length === 0
          ? <div className="rail-empty"><span className="glyph-sm" /><div className="muted">{t("chrome.rail.empty")}</div></div>
          : <div className="rail-list">{openCards.slice(0, 6).map((c) => <RailCard key={c.id} card={c} nowIso={nowIso} onDecide={decide} onGoToSource={goToCardSource} deciding={decideGuard.isDeciding(c.id)} errorMsg={decideGuard.cardErrors[c.id]} />)}</div>}
      </CardsRail>

      {needsWizard && (
        <Wizard
          onDone={({ openConnect, firstPaneMode }) => {
            setNeedsWizard(false);
            // 042 FR-004 — si el usuario eligió crear su primer pane en el paso 4, lo creamos.
            if (firstPaneMode) addPane(firstPaneMode as PaneMode);
            if (openConnect) {
              setConnectOpen(true);
            }
          }}
          // 042 FR-005 — cerrar con la X: ocultamos el wizard (re-aparece al próximo arranque, salvo
          // que el wizard ya haya escrito el fallsafe local tras un finish() fallido).
          onClose={() => setNeedsWizard(false)}
        />
      )}
      {connectOpen && (
        <ConnectScreen
          onDone={() => {
            setConnectOpen(false);
            // Refresh license-pro + license-state after first connection.
            // LOW fix (Codex): also refresh licenseState so Pro-feature gating copy is current.
            invoke<boolean>("license_is_pro").then(setProActive).catch(() => setProActive(false));
            invoke<LicenseState>("license_check").then(setLicenseState).catch((e) => { console.warn("license_check failed (will retry next interval)", e); });
          }}
        />
      )}
      {voiceOpen && <VoiceModalReal focusedPaneId={focusedPane} onClose={() => setVoiceOpen(false)} />}
      {auditDrawerOpen && (
        <AuditDrawer
          events={events}
          filter={auditFilter}
          onFilter={setAuditFilter}
          onClose={() => { setAuditDrawerOpen(false); setAuditHighlightCardId(null); }}
          highlightCardId={auditHighlightCardId}
        />
      )}
      {broadcastOpen && (
        <BroadcastModal
          panes={panes}
          onClose={() => setBroadcastOpen(false)}
        />
      )}
      {smartPasteOpen && (
        <SmartPasteModal
          focusedPaneId={focusedPane}
          onClose={() => setSmartPasteOpen(false)}
        />
      )}
      {councilOpen && <CouncilModal onClose={() => setCouncilOpen(false)} />}
      {standupOpen && <StandupModal onClose={() => setStandupOpen(false)} />}
      {prModalOpen && (
        <PrDescriptionModal
          defaultRepo={
            (typeof window !== "undefined" && (window as unknown as { __furx_repo?: string }).__furx_repo)
            || homeDir
          }
          onClose={() => setPrModalOpen(false)}
        />
      )}
      {disagreeOpen && (
        <DisagreementModal
          panes={panes}
          onClose={() => setDisagreeOpen(false)}
          onCapture={async () => snapshotBuffers()}
        />
      )}
      {shortcutSheetOpen && (
        <ShortcutSheet
          actions={buildActions({
            panes, focusedPane, proActive,
            addPane, removePane, cyclePaneMode, setFocusedPane,
            setView: (v) => navigate(v as View),
            openBroadcast: () => openModal("broadcast"),
            openVoice: () => openModal("voice"),
            openSmartPaste: () => openModal("smartpaste"),
            openStandup: () => openModal("standup"),
            openPr: () => openModal("pr"),
            openDisagree: () => openModal("disagree"),
            openCouncil: () => openModal("council"),
            setAuditDrawerOpen,
            snapshotManual: runManualSnapshot,
            readAloud,
            stopReadAloud,
            ttsAvailable,
            toggleAutoRead: () => { if (focusedPane) toggleAutoRead(focusedPane); },
            autoReadOn: focusedPane ? autoReadPanes.has(focusedPane) : false,
          })}
          onClose={() => setShortcutSheetOpen(false)}
        />
      )}
      {restoreSessions && restoreSessions.length > 0 && (
        <RestoreModal
          sessions={restoreSessions}
          onClose={() => setRestoreSessions(null)}
          /* BLOQUE H · F (PLAN_CLOSE) — wire onRestoreUi so "Restore UI only"
             actually replays panes + layout grid into the live shell state.
             The backend command returned the payload but the previous flow
             dropped it on the floor (Codex audit gap "Shell no pasa
             onRestoreUi"). */
          onRestoreUi={(payload) => {
            try {
              if (Array.isArray(payload.panes)) {
                setPanes(payload.panes.map((p) => {
                  // spec 004 F0 — preserve pane kind + data content across the bare restore path.
                  const extra = p as unknown as { kind?: "terminal" | "data" | "compare" | "web" | "context"; data_content?: string; compare_left?: string; compare_right?: string; web_url?: string; context_repo?: string; context_paths?: string };
                  const cfg: PaneCfg = {
                    id: String(p.id),
                    mode: (p.mode as PaneMode) ?? "zsh",
                    title: p.title ?? `${p.mode ?? "zsh"} restored`,
                  };
                  if (extra.kind) cfg.kind = extra.kind;
                  if (extra.data_content) cfg.data_content = extra.data_content;
                  if (extra.compare_left) cfg.compare_left = extra.compare_left;
                  if (extra.compare_right) cfg.compare_right = extra.compare_right;
                  if (extra.web_url) cfg.web_url = extra.web_url;
                  if (extra.context_repo) cfg.context_repo = extra.context_repo;
                  if (extra.context_paths) cfg.context_paths = extra.context_paths;
                  return cfg;
                }));
              }
              const layoutPanes = (payload.layout as { panes?: unknown } | null)?.panes;
              if (Array.isArray(layoutPanes)) {
                // Layout already has cwd/bundle_path; prefer it over the bare panes list.
                setPanes(layoutPanes as PaneCfg[]);
              }
              const gc = (payload.layout as { grid_cols?: string } | null)?.grid_cols;
              const gr = (payload.layout as { grid_rows?: string } | null)?.grid_rows;
              if (typeof gc === "string" && gc.length > 0) setGridCols(gc);
              if (typeof gr === "string" && gr.length > 0) setGridRows(gr);
              showToast("success", `Restored ${Array.isArray(payload.panes) ? payload.panes.length : 0} pane(s) from snapshot`);
            } catch (e) {
              console.error("restore ui payload apply", e);
              showToast("error", `Restore UI failed: ${e instanceof Error ? e.message : String(e)}`);
            }
          }}
        />
      )}
      {pendingSuggestion && (
        <SuggestionConfirm
          paneTitle={pendingSuggestion.paneTitle}
          action={pendingSuggestion.action}
          onCancel={() => setPendingSuggestion(null)}
          onConfirm={() => {
            const target = pendingSuggestion.paneId;
            const data = pendingSuggestion.action.pty_text;
            invoke("pty_write", { paneId: target, data, actionId: null, correlationId: null }).catch(console.error);
            setPendingSuggestion(null);
          }}
        />
      )}
      {paletteMode && (
        <CommandPalette
          mode={paletteMode}
          /* BLOQUE C · F10: pass the pane's actual cwd (sticky from CommandPalette,
             card flow, or set-cwd event) — previously passed pane.title which is
             a human label, NOT a real path. */
          focusedCwd={panes.find((p) => p.id === focusedPane)?.cwd}
          onClose={() => setPaletteMode(null)}
          onPickProject={(path) => {
            const target = focusedPane ?? panes[0]?.id;
            if (!target) return;
            invoke("bootstrap_compile", { paneId: target, projectDir: path }).catch(console.error);
            const ev = new CustomEvent("furx:set-cwd", { detail: { paneId: target, cwd: path } });
            window.dispatchEvent(ev);
          }}
          onPickHit={(hit) => {
            const target = focusedPane;
            if (!target) return;
            const insertion = hit.line ? `${hit.path}:${hit.line}` : hit.path;
            invoke("pty_write", { paneId: target, data: insertion, actionId: null, correlationId: null }).catch(console.error);
          }}
          onRunSpec={(args) => {
            const target = focusedPane;
            if (!target) return;
            // F20 — write `specify <args>\n` to the focused pane (works in zsh; Claude panes will see it as user input).
            const cmd = `specify ${args}\n`;
            invoke("pty_write", { paneId: target, data: cmd, actionId: null, correlationId: null }).catch(console.error);
          }}
          actions={buildActions({
            panes, focusedPane, proActive,
            addPane, removePane, cyclePaneMode, setFocusedPane,
            setView: (v) => navigate(v as View),
            openBroadcast: () => openModal("broadcast"),
            openVoice: () => openModal("voice"),
            openSmartPaste: () => openModal("smartpaste"),
            openStandup: () => openModal("standup"),
            openPr: () => openModal("pr"),
            openDisagree: () => openModal("disagree"),
            openCouncil: () => openModal("council"),
            setAuditDrawerOpen,
            snapshotManual: runManualSnapshot,
            readAloud,
            stopReadAloud,
            ttsAvailable,
            toggleAutoRead: () => { if (focusedPane) toggleAutoRead(focusedPane); },
            autoReadOn: focusedPane ? autoReadPanes.has(focusedPane) : false,
          })}
        />
      )}
      {/* US2 (spec 015) — Command Palette ⌘K universal. Window-scoped. El router
          real de deeplinks es US9; acá sólo logueamos el destino (placeholder). */}
      {cmd015Open && (
        <CommandPalette015
          onClose={() => setCmd015Open(false)}
          onNavigate={(deeplink) => navigateInternal.navigate(deeplink)}
          contextActions={contextActions}
        />
      )}
      {/* 016 US2 — Help Center (flag helpCenter). Reusa el gate del kernel vía el `invoke` envuelto. */}
      {helpEnabled && helpOpen && (
        <HelpCenter
          onClose={() => setHelpOpen(false)}
          onNavigate={(deeplink) => navigateInternal.navigate(deeplink)}
          contextSection={helpSection}
          onRelaunchTour={toursEnabled ? () => setTourActive(true) : undefined}
        />
      )}
      {/* 016 US3 — What's New (flag whatsNew). No-modal: pill en topbar + panel. */}
      {whatsNewEnabled && (
        <WhatsNew
          version={version}
          open={whatsNewOpen}
          onOpen={() => { setWhatsNewOpen(true); trackEvent("whatsnew_opened", {}); }}
          onClose={() => setWhatsNewOpen(false)}
          onNavigate={(deeplink) => navigateInternal.navigate(deeplink)}
        />
      )}
      {/* 016 US4 — Tour de primeros pasos (flag tours). Máquina de estados + a11y propias (no Modal). */}
      {toursEnabled && (
        <Tour
          active={tourActive}
          onClose={() => setTourActive(false)}
          onNavigate={(deeplink) => navigateInternal.navigate(deeplink)}
        />
      )}
      {/* 016 US4 — oferta de primer arranque (no fuerza). Aceptar → corre; descartar → marca done. */}
      {toursEnabled && tourOffered && !tourActive && (
        <div className="fxc-tour-offer" role="dialog" aria-label={t("tour.offer.title")}>
          <div className="fxc-tour-offer__text">
            <span className="fxc-tour-offer__title">{t("tour.offer.title")}</span>
            <span className="fxc-tour-offer__body">{t("tour.offer.body")}</span>
          </div>
          <div className="fxc-tour-offer__actions">
            <button type="button" className="fxc-btn" onClick={() => { setTourOffered(false); markFirstRunDone(); }}>
              {t("tour.offer.dismiss")}
            </button>
            <button type="button" className="fxc-btn fxc-btn--primary" onClick={() => { setTourOffered(false); setTourActive(true); }}>
              {t("tour.offer.start")}
            </button>
          </div>
        </div>
      )}
      {/* BLOQUE C · F7 — Inter-pane send-last modal. */}
      {interpaneSource && (
        <InterPaneSendModal
          source={interpaneSource}
          panes={panes}
          getBuffer={bufferOf}
          onDeliverToView={deliverToView}
          onClose={() => setInterpaneSource(null)}
        />
      )}
      {/* BLOQUE G · F8 — Merge review modal triggered by furx:merge-suggest. */}
      {mergeReview && (
        <MergeReviewModal
          repoPath={mergeReview.repoPath}
          branch={mergeReview.branch}
          onClose={() => setMergeReview(null)}
        />
      )}
      {/* 006 — Agent Gallery (CRUD de perfiles de agente + export/import). */}
      <AgentGallery
        open={agentGalleryOpen}
        onClose={() => setAgentGalleryOpen(false)}
        agents={agents}
        accounts={claudeAccounts}
        onChanged={reloadAgents}
        onToast={showToast}
        // 047 FR-003 — agentes que están corriendo en algún pane (borde teal en la galería).
        activeAgentIds={activeAgentIds}
      />
      {/* 008 — Orchestration Board (batch de tareas en worktrees + lanzar/revisar/merge). */}
      <OrchestrationBoard
        open={orchOpen}
        onClose={() => setOrchOpen(false)}
        agents={agents}
        onLaunch={launchOrchTask}
        onReview={(t: OrchTask) => { setOrchOpen(false); setMergeReview({ repoPath: t.repo_path, branch: t.branch }); }}
        onToast={showToast}
      />
      {/* BLOQUE A · G — global toast stack (snapshot success/error, future audit). */}
      <ToastStack toasts={toasts} onDismiss={dismissToast} />
      {/* 015 T015 — modal GLOBAL de aprobación: el gate universal del backend corta cualquier
          comando Destructive/Credential; este modal surge para CUALQUIER superficie (no sólo el
          palette) vía el wrapper lib/invoke.ts + approvalBus. */}
      <GlobalApprovalModal />
      {/* 021-voice-es · F2 — pill global de respaldo: aparece cuando el dictado está activo y el
          indicador NO puede anclarse a un pane visible. Esto incluye el caso del pane destino que
          se CERRÓ mientras grababa/transcribía (recordingPane stale): antes quedaba fantasma (ni
          ancla ni pill). La decisión vive en indicatorPlacement() (pura, testeada). */}
      {indicatorPlacement(voiceState, recordingPane ? freezeDestination(recordingPane) : { kind: "none" }, panes.map((p) => p.id)) === "globalPill" && (
        <div
          aria-live="polite"
          style={{
            position: "fixed", bottom: 22, left: "50%", transform: "translateX(-50%)",
            background: voiceState === "transcribing" ? "var(--accent)" : "var(--err)", color: "var(--bg)", padding: "8px 16px", borderRadius: 999,
            fontFamily: "var(--mono)", fontSize: 13, zIndex: 250, display: "flex", alignItems: "center", gap: 8,
            boxShadow: "0 8px 22px -10px rgba(0,0,0,.5)",
          }}
        >
          <span style={{ animation: "pulse 1s ease-in-out infinite" }}>{voiceState === "transcribing" ? "↻" : "🎙"}</span>
          {voiceState === "transcribing" ? " Transcribiendo…" : " Grabando…"}
          {voiceState === "recording" && <span style={{ opacity: .7 }}>soltá {formatPttHotkey(parsePttHotkey(pttHotkey))} · Esc cancela</span>}
        </div>
      )}
    </div>
  );
}

function PanesView({ panes, gridCols, gridRows, focusedPane, recordingPane, voiceState, claudeAccounts, agents, onFocus, onModeChange, onDataChange, onCompareChange, onWebUrlChange, onContextChange, onAdd, onRemove, onGridCols, onGridRows, onOutput, paneSuggestions, onSuggestionClick, onSendLast, onOpenAgents, onOpenOrch, bornAtOf, onStopAll }: {
  panes: PaneCfg[]; gridCols: string; gridRows: string;
  focusedPane: string | null;
  recordingPane: string | null;
  voiceState: VoiceState;
  claudeAccounts: ClaudeAccount[];
  agents: AgentProfile[];
  onFocus: (id: string) => void;
  onModeChange: (id: string, value: string) => void;
  onDataChange: (id: string, content: string) => void;
  onCompareChange: (id: string, left: string, right: string) => void;
  onWebUrlChange: (id: string, url: string) => void;
  onContextChange: (id: string, repo: string, paths: string) => void;
  onAdd: () => void; onRemove: (id: string) => void;
  onGridCols: (v: string) => void; onGridRows: (v: string) => void;
  onOutput: (paneId: string, data: string) => void;
  paneSuggestions: Record<string, Suggestion | null>;
  onSuggestionClick: (paneId: string, paneTitle: string, sug: Suggestion) => void;
  /** BLOQUE C · F7 — opens the inter-pane send modal with `pane` as the source. */
  onSendLast: (pane: PaneCfg) => void;
  /** 006 — abre la Agent Gallery (CRUD de perfiles de agente). */
  onOpenAgents: () => void;
  /** 008 — abre el Orchestration Board. */
  onOpenOrch: () => void;
  /** 047 FR-002 — epoch ms de nacimiento por pane (para el uptime del PaneCard strip). */
  bornAtOf?: (id: string) => number;
  /** 047 FR-007 — "Detener agentes": pausa todas las tareas corriendo (con confirmación). */
  onStopAll?: () => void;
}) {
  const t = useT();
  // 2×2 layout assumes exactly 4 panes; >4 falls back to auto-rows.
  const is2x2 = panes.length === 4;
  const containerRef = useRef<HTMLDivElement>(null);

  return (
    <div className="panes-shell">
      <div className="panes-toolbar">
        {/* onClick pasa el PointerEvent como 1er arg; addPane(mode) lo tomaría como `mode` (objeto sin
            .startsWith) → crash "mode.startsWith is not a function". Envolver para usar el default "zsh". */}
        <button onClick={() => onAdd()}>{t("chrome.panes.add")}</button>
        <button onClick={onOpenAgents} title={t("chrome.panes.agentsTitle")}>◆ {t("chrome.panes.agents")}</button>
        <button onClick={onOpenOrch} title={t("chrome.panes.orchestrateTitle")}>⚙ {t("chrome.panes.orchestrate")}</button>
        {/* 047 FR-007 — "Detener agentes": pausa todas las tareas corriendo. Acción HUMANA con
            confirmación (el handler pide confirm antes de invocar). NUNCA auto-disparada. */}
        {onStopAll && (
          <button onClick={onStopAll} title={t("chrome.panes.stopTitle")}>⏸ {t("chrome.panes.stop")}</button>
        )}
        <span className="muted">{t("chrome.panes.count", { count: panes.length })} · {is2x2 ? t("chrome.panes.grid2x2") : t("chrome.panes.autoLayout")}</span>
        <AttentionBadge labelOf={(pid) => panes.find((p) => p.id === pid)?.title || pid} onFocus={onFocus} />
      </div>
      <div
        ref={containerRef}
        className={`panes-grid ${is2x2 ? "grid-2x2" : "grid-auto"}`}
        style={is2x2 ? { gridTemplateColumns: gridCols, gridTemplateRows: gridRows } : undefined}
      >
        {panes.map((p) => (
          <Pane
            key={p.id}
            pane={p}
            focused={p.id === focusedPane}
            recording={p.id === recordingPane}
            voiceState={p.id === recordingPane ? voiceState : "idle"}
            suggestion={paneSuggestions[p.id] ?? null}
            claudeAccounts={claudeAccounts}
            agents={agents}
            onFocus={() => onFocus(p.id)}
            onModeChange={onModeChange}
            onDataChange={onDataChange}
            onCompareChange={onCompareChange}
            onWebUrlChange={onWebUrlChange}
            onContextChange={onContextChange}
            onRemove={() => onRemove(p.id)}
            onOutput={(data) => onOutput(p.id, data)}
            onSuggestionClick={onSuggestionClick}
            onSendLast={() => onSendLast(p)}
            bornAt={bornAtOf ? bornAtOf(p.id) : 0}
          />
        ))}
        {is2x2 && (
          <>
            <ColResizer pct={parseFrPct(gridCols)} onResize={(pct) => onGridCols(`${pct}fr ${100 - pct}fr`)} containerRef={containerRef} />
            <RowResizer pct={parseFrPct(gridRows)} onResize={(pct) => onGridRows(`${pct}fr ${100 - pct}fr`)} containerRef={containerRef} />
          </>
        )}
      </div>
    </div>
  );
}

interface PaneUsage {
  session_id: string;
  input_tokens: number;
  output_tokens: number;
  total_tokens: number;
  model: string | null;
  updated_at: string | null;
}

function fmtTokens(n: number): string {
  if (n < 1000) return String(n);
  if (n < 1_000_000) return `${(n/1000).toFixed(1)}k`;
  return `${(n/1_000_000).toFixed(2)}M`;
}

function Pane({ pane, focused, recording, voiceState, suggestion, claudeAccounts, agents, onFocus, onModeChange, onDataChange, onCompareChange, onWebUrlChange, onContextChange, onRemove, onOutput, onSuggestionClick, onSendLast, leaseWindowLabel, leaseMountInstanceId, bornAt }: {
  pane: PaneCfg; focused: boolean; recording?: boolean;
  /** 047 FR-002 — epoch ms de nacimiento del pane (para el uptime del PaneCard strip). 0 = desconocido. */
  bornAt?: number;
  /** 021-voice-es — estado del dictado anclado a este pane (idle si no es el destino). */
  voiceState?: VoiceState;
  suggestion: Suggestion | null;
  claudeAccounts: ClaudeAccount[];
  agents: AgentProfile[];
  onFocus: () => void;
  onModeChange: (id: string, value: string) => void;
  onDataChange: (id: string, content: string) => void;
  onCompareChange: (id: string, left: string, right: string) => void;
  onWebUrlChange: (id: string, url: string) => void;
  onContextChange: (id: string, repo: string, paths: string) => void; onRemove: () => void;
  onOutput: (data: string) => void;
  onSuggestionClick: (paneId: string, paneTitle: string, sug: Suggestion) => void;
  /** BLOQUE C · F7 — open the inter-pane send modal with this pane as source. */
  onSendLast: () => void;
  /** 018 Fase 2 HIGH-1 (audit) — binding del lease, sólo presente cuando el Pane vive en el
   *  WorkspaceView (flag newWorkspace ON). El Terminal los pasa a pty_write (guard fail-closed).
   *  En PanesView (legacy) quedan undefined → fail-open. */
  leaseWindowLabel?: string;
  leaseMountInstanceId?: string;
}) {
  // B9.1 — pane.mode parser: "<cli>-<slug>" o legacy "codex"/"gemini"/"aider"/"zsh".
  // Codex MED fix: "openai-api-A" debe parsear como kind="openai-api" slug="A" (NO first-dash).
  // Probamos prefixes conocidos (más largos primero) antes de fallback al first dash.
  const KIND_PREFIXES = ["openai-api-", "claude-", "codex-", "gemini-", "aider-", "custom-"] as const;
  let modeKind: string = pane.mode;
  let modeSlug: string | null = null;
  for (const p of KIND_PREFIXES) {
    if (pane.mode.startsWith(p)) {
      modeKind = p.slice(0, -1); // drop trailing dash
      modeSlug = pane.mode.slice(p.length);
      break;
    }
  }
  const cliAccount = modeSlug
    ? claudeAccounts.find((a) => a.cli_kind === modeKind && a.slug === modeSlug)
    : null;

  // BLOQUE E · F5 — per-pane Claude usage (poll every 30s; cwd-keyed).
  // Only meaningful for Claude panes with a cwd; otherwise stays null.
  const [paneUsage, setPaneUsage] = useState<PaneUsage | null>(null);
  useEffect(() => {
    let cancelled = false;
    const isClaude = modeKind === "claude";
    if (!isClaude || !pane.cwd) { setPaneUsage(null); return; }
    const tick = async () => {
      try {
        const u = await invoke<PaneUsage | null>("claude_usage_for_cwd", { cwd: pane.cwd });
        if (!cancelled) setPaneUsage(u);
      } catch {
        if (!cancelled) setPaneUsage(null);
      }
    };
    void tick();
    const id = window.setInterval(() => { void tick(); }, 30_000);
    return () => { cancelled = true; window.clearInterval(id); };
  }, [modeKind, pane.cwd]);
  // CLI kind metadata (espejo de CLI_KIND_META en types)
  const cliKindMeta: Record<string, { label: string; color: string }> = {
    claude: { label: "Claude Code", color: "var(--cyan)" },
    codex: { label: "Codex CLI", color: "var(--amber)" },
    gemini: { label: "Gemini CLI", color: "var(--green)" },
    aider: { label: "Aider", color: "var(--red)" },
    grok: { label: "Grok CLI", color: "#3bc9db" },
    "openai-api": { label: "OpenAI API", color: "var(--indigo)" },
    custom: { label: "Custom", color: "#a0b1c8" },
  };
  const kindMeta = cliKindMeta[modeKind];
  // 022 US2 — label derivado en cadena `perfil ?? cuenta.label ?? slug` (sin "A/B").
  // El ícono/color legacy de MODE_META gana solo para los modos sin slug ni perfil
  // (zsh/codex/gemini/aider) para conservar sus glifos; el resto lo deriva la función.
  const derived = derivePaneLabel(pane.mode, claudeAccounts, agents, pane.agent_profile_id);
  // 058 (ultrareview fix): se eliminó `effMode = effectivePaneMode(pane, agents)`. Ya no se usa: la key
  // y el Terminal no dependen de él (la sesión tmux deriva de orch_session||paneId; el runtime, de
  // agent_profile_id resuelto server-side). Mantenerlo forzaba remounts inútiles al cargar `agents` tarde.
  const legacyMeta = !pane.agent_profile_id ? MODE_META[pane.mode] : undefined;
  const meta = {
    icon: legacyMeta?.icon ?? derived.icon,
    color: legacyMeta?.color ?? derived.color,
    label: derived.label,
    sublabel: derived.sublabel,
    configured: derived.configured,
  };

  // Build dynamic mode list: agentes guardados + zsh + (CLI accounts por kind) + legacy CLIs
  const dynamicModes: { value: string; label: string; color?: string; group?: string }[] = [];
  // 006 — agentes guardados primero (el valor "agent:<id>" lo interpreta updateMode).
  // 066 — NO listar los agentes BUILT-IN en el dropdown del pane: duplicaban los modos CLI
  // (un "◆ Codex"/"◆ Claude · A" por cada CLI/cuenta) y metían roles caprichosos (Ventas/QA/
  // Soporte) que el usuario no creó. El dropdown muestra los modos CLI directamente; los agentes
  // built-in siguen accesibles/gestionables en la galería de Agentes. Se conserva el agente que
  // ESTE pane ya tiene asignado (sino el <select> perdería su valor actual).
  for (const a of agents) {
    if (a.is_builtin && a.id !== pane.agent_profile_id) continue;
    dynamicModes.push({
      value: `agent:${a.id}`,
      label: `${a.icon ? a.icon + " " : "◆ "}${a.name}`,
      color: a.color ?? "var(--accent)",
      group: a.is_builtin ? "Agentes · built-in" : "Agentes",
    });
  }
  dynamicModes.push({ value: "zsh", label: "zsh", color: "#6c7b91" });
  // 062 — grok NO va en este loop de CUENTAS: no es account-managed (OAuth propio, sin slug). Si
  // estuviera, una cuenta grok ofrecería "grok-<slug>" que resolve_mode no reconoce → zsh. grok se
  // ofrece SÓLO por el modo legacy "Grok (login propio)" de abajo (audit codex).
  for (const kind of ["claude", "codex", "gemini", "aider", "openai-api", "custom"] as const) {
    const km = cliKindMeta[kind];
    const accs = claudeAccounts.filter((a) => a.cli_kind === kind);
    for (const a of accs) {
      dynamicModes.push({
        value: `${kind}-${a.slug}`,
        // 062 — el display es el SLUG (nombre real de la cuenta que el user puso), NO un `label`
        // de texto libre aparte (ese defaulteaba a "Cuenta 1/2", arbitrario). Depende de la cuenta.
        label: `${km.label} · ${a.slug}${a.status !== "verified" ? " (⚠)" : ""}`,
        color: a.status === "verified" ? km.color : (a.status === "missing_token" ? "var(--red)" : "var(--amber)"),
        group: km.label,
      });
    }
  }
  // 022 US2 — si NO hay ninguna cuenta Claude, ofrecemos una entrada accionable para
  // configurar la cuenta (abre el wizard), NUNCA un placeholder "A/B" hardcodeado.
  if (claudeAccounts.filter((a) => a.cli_kind === "claude").length === 0) {
    dynamicModes.push(
      { value: "__connect__", label: "Claude (configurar cuenta…)", color: "var(--amber)", group: "Claude Code" },
    );
  }
  // Legacy modes sin slug (usan auth default del CLI / env vars existentes)
  dynamicModes.push(
    { value: "codex", label: "Codex (auth default)", color: "var(--amber)" },
    { value: "gemini", label: "Gemini (auth default)", color: "var(--green)" },
    { value: "aider", label: "Aider (config default)", color: "var(--red)" },
    { value: "grok", label: "Grok (login propio)", color: "#3bc9db" }, // 062 — auth por `grok login` (OAuth)
  );
  // spec 004 F3/F4 — non-terminal pane kinds, selectable from the same dropdown.
  dynamicModes.push({ value: "__data__", label: "📊 Data viewer (JSON/CSV)", color: "var(--accent)" });
  dynamicModes.push({ value: "__compare__", label: "⊟ Compare responses", color: "var(--accent)" });
  dynamicModes.push({ value: "__web__", label: "🌐 Web (pinned URL)", color: "var(--accent)" });
  dynamicModes.push({ value: "__context__", label: "🗂 Project context", color: "var(--accent)" });
  const isData = pane.kind === "data";
  const isCompare = pane.kind === "compare";
  const isWeb = pane.kind === "web";
  const isContext = pane.kind === "context";
  const selectValue = pane.agent_profile_id ? `agent:${pane.agent_profile_id}`
    : isData ? "__data__" : isCompare ? "__compare__" : isWeb ? "__web__" : isContext ? "__context__" : pane.mode;
  return (
    <div className={`pane ${focused ? "focused" : ""} ${recording ? "recording" : ""}`} onClick={onFocus}>
      <div className="pane-header">
        <span className="pane-icon" style={{ background: `${meta.color}26`, color: meta.color, borderColor: `${meta.color}55` }}>{meta.icon}</span>
        <span className="pane-title">{pane.title}</span>
        {/* 022 US2 — cuenta no configurada: estado honesto + acción (abre el wizard),
            NUNCA "A/B". El badge de cuenta real (más abajo) cubre el caso configurado;
            cuando hay perfil asignado mostramos el label derivado del perfil. */}
        {/* 066 — cuenta SIN configurar: CTA accionable (abre el wizard). El label de modo/cuenta
            del caso CONFIGURADO se eliminó de acá: ya lo muestra el dropdown (única fuente), sin
            repetirlo 4 veces ni exponer el slug interno (A/B). */}
        {!meta.configured && (
          <button
            type="button"
            className="pane-account-badge"
            title="Configurá una cuenta para este pane"
            onClick={(e) => { e.stopPropagation(); onModeChange(pane.id, "__connect__"); }}
            style={{ background: "var(--amber)22", borderColor: "var(--amber)55", color: "var(--amber)", cursor: "pointer" }}
          >
            ⚠ Configurar cuenta
          </button>
        )}
        {/* 021-voice-es — indicador de dictado ANCLADO a este pane (el destino congelado).
            Grabando → Transcribiendo → desaparece al insertar. Tokens V3, dark+light. */}
        {voiceState && voiceState !== "idle" && (
          <span
            className={`pane-voice-anchor ${voiceState === "transcribing" ? "transcribing" : "recording"}`}
            role="status"
            aria-live="polite"
            title={`Dictado → ${pane.title}`}
          >
            <span className="pva-icon">{voiceState === "transcribing" ? "↻" : "🎙"}</span>
            <span className="pva-arrow">→ {pane.title}</span>
            <span className="pva-state">{stateLabel(voiceState)}</span>
          </span>
        )}
        {/* 066 — badge de cuenta ("Claude · A") ELIMINADO: redundante con el dropdown + exponía
            el slug interno A/B + se cruzaba en panes de otro CLI (agent_profile_id ≠ pane.mode). */}
        {paneUsage && (
          /* BLOQUE E · F5 — per-pane Claude tokens read from
             ~/.claude/projects/<encoded-cwd>/<session>/usage.json */
          <span
            className="pane-account-badge"
            title={`Session ${paneUsage.session_id} · ${paneUsage.input_tokens.toLocaleString()} in / ${paneUsage.output_tokens.toLocaleString()} out${paneUsage.model ? ` · ${paneUsage.model}` : ""}${paneUsage.updated_at ? ` · updated ${paneUsage.updated_at}` : ""}`}
            style={{
              background: "var(--cyan-glow)",
              borderColor: "var(--cyan-strong)",
              color: "var(--cyan)",
              fontVariantNumeric: "tabular-nums",
            }}
          >
            tok {fmtTokens(paneUsage.total_tokens)}
          </span>
        )}
        {suggestion && (
          <button
            type="button"
            className={`suggest-badge sg-${suggestion.kind}`}
            title={suggestion.hint}
            aria-label={`Suggestion ${suggestion.label}: ${suggestion.hint}`}
            onClick={(e) => { e.stopPropagation(); onSuggestionClick(pane.id, pane.title, suggestion); }}
          >
            {suggestion.label}
          </button>
        )}
        <select
          className="pane-mode-select"
          value={selectValue}
          onChange={(e) => onModeChange(pane.id, e.target.value)}
          onClick={(e) => e.stopPropagation()}
          title="Cambiar modo / tipo de panel (recrea el panel)"
        >
          {dynamicModes.map((m) => <option key={m.value} value={m.value}>{m.label}</option>)}
        </select>
        <span
          className="pane-status"
          role="img"
          aria-label={`Pane ${pane.id.slice(0, 6)} active · mode ${pane.mode}${cliAccount ? ` · ${cliAccount.slug}` : ""}`}
          title={`Pane ${pane.id.slice(0, 6)} · mode ${pane.mode}${cliAccount ? ` · ${cliAccount.slug}` : ""}`}
        >
          ●
        </span>
        <button
          className="pane-send"
          onClick={(e) => { e.stopPropagation(); onSendLast(); }}
          title="Enviar último output a otro panel"
          aria-label={`Enviar último output de ${pane.title} a otro panel`}
          style={{ marginRight: 4 }}
        >
          →
        </button>
        {/* 018 Fase 2 US2 — detach/re-attach. Sólo cuando el pane vive en el WorkspaceView
            (leaseWindowLabel presente): en PanesView legacy NO hay multi-window. */}
        {leaseWindowLabel && (
          <DetachButton panelId={pane.id} windowLabel={leaseWindowLabel} />
        )}
        <button className="pane-close" onClick={(e) => { e.stopPropagation(); onRemove(); }} title="Cerrar panel (⌘W)">×</button>
      </div>
      {/* 047 FR-002 — cabecera contextual de ~28px (modo · tokens · tiempo · estado) + overlay
          "Aprobar" cuando el pane reclama decisión humana. Sólo para panes de terminal (los kinds
          data/compare/web/context no tienen proceso ni atención). "Aprobar" = acción humana: enfoca
          el pane (onFocus), NUNCA auto-aprueba. */}
      {!isData && !isCompare && !isWeb && !isContext && (
        <PaneCardStrip
          paneId={pane.id}
          modeLabel={null}
          modeColor={meta.color}
          tokens={paneUsage ? fmtTokens(paneUsage.total_tokens) : null}
          bornAt={bornAt ?? 0}
          /* hasLiveProcess: INFORMATIVO — un pane de terminal cuenta como vivo hasta cerrarse (el
             front no tiene una señal de PTY-vivo por-pane acá; el "terminó" real lo da la atención). */
          hasLiveProcess
          onApprove={onFocus}
        />
      )}
      <div className="pane-body">
        {/* spec 004 F0: a data pane renders the viewer (no PTY); otherwise the Terminal.
            `key` ata el terminal a su identidad: cambiar mode/cwd/agent_profile_id/orch_session
            des-monta y re-monta (mata PTY viejo, spawnea nuevo). BLOQUE B · F2: cwd sticky for restore.
            058 (ultrareview fix): se QUITÓ `effMode` de la key. Tras 056 la sesión tmux deriva de
            `orch_session || paneId` (no de effMode) y el runtime lo resuelve el backend por
            `agent_profile_id` contra su db (no depende del `agents` del front). Cuando `agents` llega
            tarde y effMode flippea legacy→synth, NI la sesión NI el comando cambian → remontar era un
            flash + recaptura de 3000 líneas inútiles. Las deps reales bastan. */}
        {isData ? (
          <DataViewer
            initialContent={pane.data_content}
            onContentChange={(c) => onDataChange(pane.id, c)}
          />
        ) : isCompare ? (
          <ComparatorView
            initialLeft={pane.compare_left}
            initialRight={pane.compare_right}
            onChange={(l, r) => onCompareChange(pane.id, l, r)}
          />
        ) : isWeb ? (
          <WebPane url={pane.web_url} onPin={(u) => onWebUrlChange(pane.id, u)} />
        ) : isContext ? (
          <ContextPane repo={pane.context_repo} paths={pane.context_paths} onChange={(r, ps) => onContextChange(pane.id, r, ps)} />
        ) : (
          <Terminal key={`${pane.id}::${pane.mode}::${pane.cwd ?? ""}::${pane.agent_profile_id ?? ""}::${pane.orch_session ?? ""}`} paneId={pane.id} mode={pane.mode} cwd={pane.cwd} agentProfileId={pane.agent_profile_id} sessionOverride={pane.orch_session} onOutput={onOutput} leaseWindowLabel={leaseWindowLabel} leaseMountInstanceId={leaseMountInstanceId} />
        )}
      </div>
    </div>
  );
}

// Parsea el porcentaje del 1er track de un grid-template (ej "70fr 30fr" → 70). Default 50.
function parseFrPct(tpl: string): number {
  // El % del 1er track = primerFr / sumaDeTodosLosFr × 100 (audit-3 Codex: antes tomaba solo el 1er
  // valor y lo clampeaba, así "1fr 1fr" daba 20 en vez de 50, y "7fr 3fr" daba 20 en vez de 70).
  const frs = [...tpl.matchAll(/(-?\d+(?:\.\d+)?)\s*fr/g)]
    .map((m) => parseFloat(m[1]))
    .filter((n) => Number.isFinite(n));
  if (frs.length < 2) return 50;
  const sum = frs.reduce((a, b) => a + b, 0);
  if (sum <= 0) return 50;
  return Math.max(20, Math.min(80, (frs[0] / sum) * 100));
}

// Resizer entre las 2 columnas del grid 2×2. Vive sobre el grid en posición absoluta.
function ColResizer({ pct, onResize, containerRef }: { pct: number; onResize: (pct: number) => void; containerRef: React.RefObject<HTMLDivElement | null> }) {
  const [dragging, setDragging] = useState(false);
  useEffect(() => {
    if (!dragging) return;
    const onMove = (e: MouseEvent) => {
      const el = containerRef.current; if (!el) return;
      const rect = el.getBoundingClientRect();
      const pct = Math.max(20, Math.min(80, ((e.clientX - rect.left) / rect.width) * 100));
      onResize(pct);
    };
    const onUp = () => setDragging(false);
    window.addEventListener("mousemove", onMove);
    window.addEventListener("mouseup", onUp);
    return () => { window.removeEventListener("mousemove", onMove); window.removeEventListener("mouseup", onUp); };
  }, [dragging, onResize, containerRef]);
  return (
    <div className="col-resizer" style={{ left: `${pct}%` }} onMouseDown={() => setDragging(true)} title="Drag para redimensionar columnas" />
  );
}

function RowResizer({ pct, onResize, containerRef }: { pct: number; onResize: (pct: number) => void; containerRef: React.RefObject<HTMLDivElement | null> }) {
  const [dragging, setDragging] = useState(false);
  useEffect(() => {
    if (!dragging) return;
    const onMove = (e: MouseEvent) => {
      const el = containerRef.current; if (!el) return;
      const rect = el.getBoundingClientRect();
      const pct = Math.max(20, Math.min(80, ((e.clientY - rect.top) / rect.height) * 100));
      onResize(pct);
    };
    const onUp = () => setDragging(false);
    window.addEventListener("mousemove", onMove);
    window.addEventListener("mouseup", onUp);
    return () => { window.removeEventListener("mousemove", onMove); window.removeEventListener("mouseup", onUp); };
  }, [dragging, onResize, containerRef]);
  return (
    <div className="row-resizer" style={{ top: `${pct}%` }} onMouseDown={() => setDragging(true)} title="Drag para redimensionar filas" />
  );
}

async function openInClaude(cardId: string) {
  try {
    const spec = await invoke<{ bundle_path: string | null; project_dir: string | null; suggested_mode: string }>("card_open_in_claude", { cardId });
    // BLOQUE B · B: forward card_id alongside paneSpec so the listener can
    // inject it into the initial Claude prompt (Codex must-fix #6).
    const ev = new CustomEvent("furx:dispatch-open-card", { detail: { paneSpec: { ...spec, card_id: cardId } } });
    window.dispatchEvent(ev);
  } catch (e) {
    console.error("open in claude failed", e);
  }
}

// 022 P1 · US6 — tipo de decisión que el inbox emite (incluye snooze con duración explícita).
type DecideFn = (id: string, decision: string, snoozeUntil?: string) => void;

/** Etiqueta i18n de severidad (sentence-case, derivada del catálogo). */
function severityLabel(t: ReturnType<typeof useT>, sev: string): string {
  if (sev === "critical") return t("incidents.sev.critical");
  if (sev === "warning") return t("incidents.sev.warning");
  return t("incidents.sev.info");
}

/**
 * 022 P1 · US6 — menú de snooze con opciones (1h / 4h / mañana), NO un snooze fijo. Calcula el
 * `snooze_until` puro (lib/incidents) y lo pasa a `onDecide`. Cierra al elegir, al perder foco o Esc.
 */
function SnoozeMenu({ onSnooze, compact, disabled }: { onSnooze: (snoozeUntil: string) => void; compact?: boolean; disabled?: boolean }) {
  const t = useT();
  const [open, setOpen] = useState(false);
  const ref = useRef<HTMLDivElement | null>(null);
  useEffect(() => {
    if (!open) return;
    const onDoc = (e: MouseEvent) => { if (ref.current && !ref.current.contains(e.target as Node)) setOpen(false); };
    const onKey = (e: KeyboardEvent) => { if (e.key === "Escape") setOpen(false); };
    document.addEventListener("mousedown", onDoc);
    document.addEventListener("keydown", onKey);
    return () => { document.removeEventListener("mousedown", onDoc); document.removeEventListener("keydown", onKey); };
  }, [open]);
  // 044 FR-003 (audit-3 fix MED): si el menú estaba ABIERTO y la card pasa a "en vuelo" (disabled),
  // cerrarlo → sus opciones no pueden dispararse (evita una 2ª decisión durante el invoke).
  useEffect(() => { if (disabled) setOpen(false); }, [disabled]);
  const pick = (opt: SnoozeOption) => {
    if (disabled) return; // guard extra: no decidir si la card está en vuelo.
    onSnooze(computeSnoozeUntil(opt, Date.now()));
    setOpen(false);
  };
  const optLabel = (opt: SnoozeOption) =>
    opt === "1h" ? t("incidents.snooze.1h") : opt === "4h" ? t("incidents.snooze.4h") : t("incidents.snooze.tomorrow");
  return (
    <div className="snooze-menu" ref={ref} style={{ position: "relative", display: "inline-block" }}>
      <button
        type="button"
        className={compact ? "mini" : "ghost"}
        aria-haspopup="menu"
        aria-expanded={open}
        aria-label={t("incidents.snooze.menuAria")}
        title={t("incidents.action.snooze")}
        disabled={disabled}
        onClick={() => setOpen((v) => !v)}
      >
        {compact ? t("chrome.rail.cardSnooze") : t("incidents.action.snooze")} ▾
      </button>
      {open && !disabled && (
        <div className="snooze-menu-pop" role="menu" aria-label={t("incidents.snooze.menuAria")}>
          {SNOOZE_OPTIONS.map((opt) => (
            <button key={opt} type="button" role="menuitem" className="snooze-menu-item" disabled={disabled} onClick={() => pick(opt)}>
              {optLabel(opt)}
            </button>
          ))}
        </div>
      )}
    </div>
  );
}

/**
 * 022 P1 · US6 — slide-over de detalle del incidente: TODO lo que se sabe (título, proyecto,
 * severidad, fuente, timestamp, causa, estado). Accesible (role=dialog, foco atrapado básico, Esc).
 */
function IncidentSlideOver({ card, nowIso, onClose, onDecide, onGoToSource, onOpenAudit, deciding, errorMsg }: {
  card: Card; nowIso: string; onClose: () => void; onDecide: DecideFn; onGoToSource: (c: Card) => void;
  // 047 FR-004 — trazabilidad: abrir el drawer de audit con el evento de esta card resaltado.
  onOpenAudit?: (cardId: string) => void;
  // 044 FR-003 — si la card está en vuelo (decidida desde la lista), las acciones de decisión del
  // slide-over quedan deshabilitadas y muestra el error inline.
  deciding?: boolean; errorMsg?: string;
}) {
  const t = useT();
  const closeRef = useRef<HTMLButtonElement | null>(null);
  useEffect(() => {
    closeRef.current?.focus();
    const onKey = (e: KeyboardEvent) => { if (e.key === "Escape") onClose(); };
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, [onClose]);
  const target = sourceTarget(card);
  return (
    <div className="slideover-backdrop" onMouseDown={onClose}>
      <aside
        className="slideover"
        role="dialog"
        aria-modal="true"
        aria-label={t("incidents.detail.title")}
        onMouseDown={(e) => e.stopPropagation()}
      >
        <header className="slideover-header">
          <h2 className="slideover-title">{t("incidents.detail.title")}</h2>
          <button ref={closeRef} type="button" className="ghost" aria-label={t("incidents.detail.closeAria")} title={t("common.close")} onClick={onClose}>✕</button>
        </header>
        <div className="slideover-body">
          <div className="slideover-card-title">
            {card.title}
            {card.reopened ? <span className="badge-reopened" title={t("incidents.reopenedHint")}> {t("incidents.reopened")}</span> : null}
          </div>
          <dl className="slideover-dl">
            <dt>{t("incidents.detail.project")}</dt><dd>{card.project}</dd>
            <dt>{t("incidents.detail.source")}</dt><dd>{card.source}</dd>
            <dt>{t("incidents.detail.severity")}</dt><dd><span className={`sev-tag sev-${card.severity}`}>{severityLabel(t, card.severity)}</span></dd>
            <dt>{t("incidents.detail.created")}</dt><dd>{fmtWhen(card.created_at)}</dd>
            <dt>{t("incidents.detail.status")}</dt><dd>{inboxState(card, nowIso)}</dd>
            <dt>{t("incidents.detail.cause")}</dt><dd>{card.cause || <span className="muted">{t("incidents.detail.noCause")}</span>}</dd>
          </dl>
          {isSnoozed(card, nowIso) && card.snooze_until && (
            <div className="muted slideover-snooze">{t("incidents.snoozedUntil", { when: fmtWhen(card.snooze_until) })}</div>
          )}
          {errorMsg && <div className="card-error" role="alert">{errorMsg}</div>}
        </div>
        <footer className="slideover-actions">
          {target.view && (
            <button type="button" className="primary" onClick={() => { onGoToSource(card); onClose(); }}>
              {t(target.labelKey)}
            </button>
          )}
          <button type="button" onClick={() => { openInClaude(card.id); onClose(); }}>{t("incidents.action.openClaude")}</button>
          {/* 047 FR-004 — ver el rastro de audit relacionado con esta card (trazabilidad). */}
          {onOpenAudit && (
            <button type="button" onClick={() => { onOpenAudit(card.id); onClose(); }} title="Ver el evento de auditoría relacionado">
              Ver en audit
            </button>
          )}
          {/* Acciones de DECISIÓN: deshabilitadas si la card ya está en vuelo (decidida desde la lista). */}
          <SnoozeMenu onSnooze={(until) => { onDecide(card.id, "snoozed", until); onClose(); }} disabled={deciding} />
          <button type="button" disabled={deciding} onClick={() => { onDecide(card.id, "dismissed"); onClose(); }}>{t("incidents.action.dismiss")}</button>
          <button type="button" className="primary" disabled={deciding} onClick={() => { onDecide(card.id, "approved"); onClose(); }}>{t("incidents.action.approve")}</button>
          <button type="button" className="danger" disabled={deciding} onClick={() => { onDecide(card.id, "rejected"); onClose(); }}>{t("incidents.action.reject")}</button>
        </footer>
      </aside>
    </div>
  );
}

export function CardsView({ cards, onDecide, onGoToSource, onOpenAudit, initialOpenOnly, onClearFilter, loading, error, onRetry, isDeciding, cardErrors }: {
  cards: Card[];
  onDecide: DecideFn;
  onGoToSource: (c: Card) => void;
  // 047 FR-004 — abrir el drawer de audit con el evento de la card resaltado (trazabilidad).
  onOpenAudit?: (cardId: string) => void;
  // 022 P0b — drill-down: si el stat "Incidentes abiertos" nos trajo, arrancar en "solo accionables".
  initialOpenOnly?: boolean;
  onClearFilter?: () => void;
  // 044 FR-002 — estados de carga del inbox: skeleton mientras el invoke está en vuelo y NO hay
  // datos aún; banner de error + "Reintentar" si el fetch falló. `loading`/`error` los maneja el Shell.
  loading?: boolean;
  error?: string | null;
  onRetry?: () => void;
  // 044 FR-003 — guard anti doble-respuesta: predicado de card en vuelo (varias a la vez) + error por card.
  isDeciding?: (cardId: string) => boolean;
  cardErrors?: Record<string, string>;
}) {
  const t = useT();
  // "solo accionables" (FR-011): excluye snoozed/dismissed/closed. Off = inbox visible (incl. read).
  const [actionableOnly, setActionableOnly] = useState<boolean>(!!initialOpenOnly);
  // 050 FR-004 — modo compacto: densidad alta opcional (1 línea por card). Persistido en localStorage
  // (default OFF → la vista normal no cambia para nadie; cero regresión). Solo afecta el render.
  const [compact, setCompact] = useState<boolean>(() => loadCompactIncidents());
  const toggleCompact = () => setCompact((prev) => { const next = !prev; saveCompactIncidents(next); return next; });
  const [groupBy, setGroupBy] = useState<GroupBy>("project");
  const [detailCard, setDetailCard] = useState<Card | null>(null);
  const [focusedId, setFocusedId] = useState<string | null>(null);
  // reloj para el cálculo del inbox (snooze/expiración) sin esperar un refetch.
  const [now, setNow] = useState<number>(Date.now());
  useEffect(() => { const id = window.setInterval(() => setNow(Date.now()), 5000); return () => window.clearInterval(id); }, []);
  const nowIso = useMemo(() => new Date(now).toISOString().slice(0, 19).replace("T", " "), [now]);
  useEffect(() => { setActionableOnly(!!initialOpenOnly); }, [initialOpenOnly]);

  const visible = useMemo(() => inboxCards(cards, nowIso, actionableOnly), [cards, nowIso, actionableOnly]);
  const groups = useMemo(() => groupIncidents(visible, groupBy, nowIso), [visible, groupBy, nowIso]);
  // 044 FR-002 (audit-3 fix) — namespace de claves por `groupBy` (proyecto "critical" ≠ severidad
  // "critical") + cap GLOBAL de 200 en el DOM + `flat` derivado SÓLO de cards visibles.
  const nsKey = (groupKey: string) => `${groupBy}:${groupKey}`;

  // 044 FR-002 — estado de colapso por grupo (persistido). Primer arranque: critical expandido, resto
  // colapsado. Lo persistido por el usuario MANDA. Claves namespaced por groupBy (audit-3 fix).
  const [collapsed, setCollapsed] = useState<Record<string, boolean>>({});
  // audit-3 fix (LOW): firma inequívoca de las claves de grupo (JSON, no join(" ") — `["a b"]` vs
  // `["a","b"]` colisionaban) para disparar el efecto cuando cambia el SET de grupos.
  const groupKeysSig = JSON.stringify(groups.map((g) => g.key));
  useEffect(() => {
    // audit-3 fix: re-consultar SIEMPRE lo persistido (no solo `prev`) -> un grupo que desaparece y
    // reaparece (cambio de filtro/datos) recupera la preferencia del usuario en vez de caer al default.
    // `prev` (memoria) prioriza sobre persistido (toggles de esta sesion); persistido cubre grupos
    // ausentes de `prev`.
    setCollapsed((prev) => {
      const persisted = loadCollapsedState() ?? {};
      const seed: Record<string, boolean> = { ...persisted, ...prev };
      const nsGroups = groups.map((g) => ({ ...g, key: nsKey(g.key) }));
      return initialCollapsedState(nsGroups, seed);
    });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [groupBy, groupKeysSig]);

  const toggleGroup = (groupKey: string) => {
    const k = nsKey(groupKey);
    setCollapsed((prev) => {
      const next = { ...prev, [k]: !(prev[k] ?? false) };
      // audit-3 fix (HIGH): persistir MERGEANDO sobre el mapa COMPLETO guardado, no sobre el `collapsed`
      // en memoria (que sólo tiene los grupos visibles ahora). Si guardáramos `next` crudo, perderíamos
      // las preferencias de grupos que no están en pantalla en este momento. El persistido + el toggle
      // nuevo = estado completo. Best-effort: si falla (quota), el estado vive en memoria igual.
      const fullPersisted = { ...(loadCollapsedState() ?? {}), [k]: next[k] };
      saveCollapsedState(fullPersisted);
      return next;
    });
  };

  // 044 FR-002 — cuantas cards se muestran por grupo (primeras 5; "ver mas" suma 50). El cap 200 es
  // GLOBAL al DOM (audit-3 fix): se reparte recorriendo los grupos en orden hasta agotar el budget.
  const [visibleCount, setVisibleCount] = useState<Record<string, number>>({});
  const showMore = (groupKey: string) => {
    setVisibleCount((prev) => {
      const k = nsKey(groupKey);
      const cur = prev[k] ?? INCIDENT_GROUP_INITIAL_VISIBLE;
      return { ...prev, [k]: Math.min(cur + INCIDENT_GROUP_VISIBLE_STEP, INCIDENT_GROUP_DOM_CAP) };
    });
  };

  // 044 FR-002 (audit-3 fix) — plan de render con CAP GLOBAL de 200 cards en el DOM. Recorremos los
  // grupos en orden; cada grupo expandido pide min(visibleCount, cards.length), sin exceder el budget
  // global. Un grupo colapsado NO consume budget. De aca sale TAMBIEN el `flat` de la nav por teclado
  // -> j/k/e/x solo operan sobre cards VISIBLES (no colapsadas ni mas alla del cap).
  const renderPlan = useMemo(() => {
    let budget = INCIDENT_GROUP_DOM_CAP;
    return groups.map((g) => {
      const isCollapsed = collapsed[nsKey(g.key)] ?? !groupHasCritical(g);
      if (isCollapsed) {
        return { group: g, isCollapsed, rendered: [] as Card[], remaining: 0 };
      }
      const want = Math.min(visibleCount[nsKey(g.key)] ?? INCIDENT_GROUP_INITIAL_VISIBLE, g.cards.length);
      const take = Math.min(want, budget);
      budget -= take;
      const rendered = g.cards.slice(0, take);
      // cuantas mas montaria el proximo "ver mas", acotado por el budget global y por el step.
      const remaining = Math.min(g.cards.length - take, budget, INCIDENT_GROUP_VISIBLE_STEP);
      return { group: g, isCollapsed, rendered, remaining };
    });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [groups, collapsed, visibleCount, groupBy]);

  // Navegacion por teclado: SOLO sobre las cards efectivamente renderizadas (visibles en el DOM).
  const flat = useMemo(() => renderPlan.flatMap((p) => p.rendered), [renderPlan]);
  // El slide-over abierto refleja la última versión de la card (auto-unsnooze/reopened en vivo).
  const detail = detailCard ? (cards.find((c) => c.id === detailCard.id) ?? detailCard) : null;

  // 022 P1 · US6 — keyboard-first: triar la card enfocada. e=approve, h=snooze(1h), x=dismiss,
  // r=mark-read, o=ir al origen, j/k mueven el foco, Enter abre el detalle. No interfiere con
  // inputs/modales (guard: ignora si hay un slide-over abierto o el foco está en un control de texto).
  useEffect(() => {
    const handler = (e: KeyboardEvent) => {
      if (detailCard) return; // el slide-over tiene su propio Esc.
      // Codex MED-2: el guard ignoraba inputs/selects pero NO botones ni contentEditable, así que
      // Enter/Space con foco en un botón de card burbujeaban al handler global (abría detalle /
      // preventDefault) en vez de activar el botón. Ignorar TODO control interactivo nativo.
      const el = e.target as HTMLElement | null;
      const tag = el?.tagName;
      if (tag === "INPUT" || tag === "TEXTAREA" || tag === "SELECT" || tag === "BUTTON" || tag === "A") return;
      if (el?.isContentEditable) return;
      if (el?.closest("button, a, [role=\"button\"], [contenteditable=\"true\"]")) return;
      if (e.metaKey || e.ctrlKey || e.altKey) return;
      if (flat.length === 0) return;
      const idx = Math.max(0, flat.findIndex((c) => c.id === focusedId));
      const cur = flat[idx];
      switch (e.key) {
        case "j": e.preventDefault(); setFocusedId(flat[Math.min(flat.length - 1, idx + 1)]?.id ?? null); break;
        case "k": e.preventDefault(); setFocusedId(flat[Math.max(0, idx - 1)]?.id ?? null); break;
        case "e": if (cur) { e.preventDefault(); onDecide(cur.id, "approved"); } break;
        case "x": if (cur) { e.preventDefault(); onDecide(cur.id, "dismissed"); } break;
        case "r": if (cur) { e.preventDefault(); onDecide(cur.id, "read"); } break;
        case "h": if (cur) { e.preventDefault(); onDecide(cur.id, "snoozed", computeSnoozeUntil("1h", Date.now())); } break;
        case "o": if (cur) { e.preventDefault(); if (sourceTarget(cur).view) onGoToSource(cur); else setDetailCard(cur); } break;
        case "Enter": if (cur) { e.preventDefault(); setDetailCard(cur); } break;
      }
    };
    window.addEventListener("keydown", handler);
    return () => window.removeEventListener("keydown", handler);
  }, [flat, focusedId, detailCard, onDecide, onGoToSource]);

  const total = cards.length;
  const actionableCount = useMemo(() => inboxCards(cards, nowIso, true).length, [cards, nowIso]);

  // 044 FR-002 — skeleton mientras el primer fetch está en vuelo y todavía NO hay datos. (Si ya hay
  // cards de un fetch previo, NO mostramos skeleton: un refresh en background no debe vaciar la vista.)
  if (loading && total === 0 && !error) {
    return (
      <div className="page">
        <div className="page-header"><div className="page-title">{t("incidents.title")}</div></div>
        <div className="incidents-skeleton" aria-busy="true" aria-label={t("incidents.loading")}>
          {[0, 1, 2].map((i) => (
            <div key={i} className="card-item skeleton-card" aria-hidden="true">
              <div className="skeleton-line skeleton-line-title" />
              <div className="skeleton-line skeleton-line-meta" />
            </div>
          ))}
        </div>
      </div>
    );
  }

  // 044 FR-002 — banner de error inline + "Reintentar" cuando el fetch falló y no tenemos datos.
  if (error && total === 0) {
    return (
      <div className="page">
        <div className="page-header"><div className="page-title">{t("incidents.title")}</div></div>
        <div className="incidents-error" role="alert">
          <div className="head">{t("incidents.error.head")}</div>
          <div className="body muted">{error}</div>
          {onRetry && (
            <button type="button" className="primary" onClick={onRetry}>{t("incidents.error.retry")}</button>
          )}
        </div>
      </div>
    );
  }

  if (total === 0) {
    return (
      <div className="page">
        <div className="page-header"><div className="page-title">{t("incidents.title")}</div></div>
        <div className="empty"><span className="glyph" /><div className="head">{t("incidents.empty.head")}</div><div className="body muted">{t("incidents.empty.body")}</div></div>
      </div>
    );
  }

  return (
    <div className="page">
      <div className="page-header">
        <div className="page-title">{t("incidents.title")}</div>
        <div className="page-sub">{t("incidents.sub", { total, open: actionableCount })}</div>
        <div className="page-actions incidents-toolbar">
          <button
            type="button"
            className={actionableOnly ? "ghost is-active" : "ghost"}
            aria-pressed={actionableOnly}
            onClick={() => { const next = !actionableOnly; setActionableOnly(next); if (!next) onClearFilter?.(); }}
          >
            {actionableOnly ? t("incidents.filter.actionable") : t("incidents.filter.all")}
          </button>
          <label className="incidents-group-select">
            <span className="muted">{t("incidents.group.label")}</span>
            <select value={groupBy} onChange={(e) => setGroupBy(e.target.value as GroupBy)} aria-label={t("incidents.group.label")}>
              <option value="project">{t("incidents.group.project")}</option>
              <option value="severity">{t("incidents.group.severity")}</option>
              <option value="source">{t("incidents.group.source")}</option>
            </select>
          </label>
          {/* 050 FR-004 — modo compacto (densidad alta). Persistido; default OFF → cero regresión. */}
          <button
            type="button"
            className={compact ? "ghost is-active" : "ghost"}
            aria-pressed={compact}
            onClick={toggleCompact}
            title={t("incidents.compact.title")}
          >
            {t("incidents.compact.label")}
          </button>
        </div>
      </div>
      {/* 044 FR-002 — si un refresh en background falló pero TENEMOS datos viejos, avisamos sin vaciar
          la vista (banner inline no-bloqueante con "Reintentar"). */}
      {error && total > 0 && (
        <div className="incidents-error-inline" role="alert">
          <span className="muted">{t("incidents.error.head")}</span>
          {onRetry && (
            <button type="button" className="ghost" onClick={onRetry}>{t("incidents.error.retry")}</button>
          )}
        </div>
      )}
      {visible.length === 0 ? (
        <div className="empty"><span className="glyph" /><div className="head">{t("incidents.empty.head")}</div><div className="body muted">{t("incidents.empty.actionable")}</div></div>
      ) : (
        <div className={compact ? "incidents-groups is-compact" : "incidents-groups"}>
          {renderPlan.map(({ group: g, isCollapsed, rendered, remaining }) => {
            // 044 FR-002 — grupo vacío no se renderiza (defensa extra; groupIncidents no emite vacíos).
            if (g.cards.length === 0) return null;
            const hasCritical = groupHasCritical(g);
            const groupLabel = groupBy === "severity" ? severityLabel(t, g.key) : g.key;
            const bodyId = `incidents-group-body-${groupBy}-${g.key}`;
            const sev = g.cards[0]?.severity ?? "info";
            return (
              <section key={`${groupBy}:${g.key}`} className={`incidents-group sev-group-${sev} ${isCollapsed ? "is-collapsed" : ""}`}>
                <header className="incidents-group-header">
                  <button
                    type="button"
                    className="incidents-group-toggle"
                    aria-expanded={!isCollapsed}
                    aria-controls={bodyId}
                    aria-label={t("incidents.group.toggleAria", { group: groupLabel })}
                    onClick={() => toggleGroup(g.key)}
                  >
                    <span className="incidents-group-caret" aria-hidden="true">{isCollapsed ? "▸" : "▾"}</span>
                    <span className="incidents-group-key">{groupLabel}</span>
                  </button>
                  {/* Badge de severidad del grupo (color por la card más urgente del grupo, que va 1ra). */}
                  <span className={`sev-tag sev-${sev}`}>{severityLabel(t, sev)}</span>
                  {/* Badge de emergencia: visible AUNQUE el grupo esté colapsado, si contiene un critical. */}
                  {hasCritical && (
                    <span className="badge-emergency" title={t("incidents.group.emergencyHint")}>{t("incidents.group.emergency")}</span>
                  )}
                  <span className="muted incidents-group-count">{t("incidents.group.count", { count: g.actionableCount })}</span>
                </header>
                {!isCollapsed && (
                  <div className="card-list" id={bodyId}>
                    {rendered.map((c) => (
                      <CardItem
                        key={c.id}
                        card={c}
                        nowIso={nowIso}
                        focused={focusedId === c.id}
                        onDecide={onDecide}
                        onGoToSource={onGoToSource}
                        onShowDetail={() => setDetailCard(c)}
                        onFocusCard={() => setFocusedId(c.id)}
                        deciding={isDeciding?.(c.id)}
                        errorMsg={cardErrors?.[c.id]}
                      />
                    ))}
                    {/* "Ver N más": N es lo que el próximo click MONTARÁ (acotado por step + cap global). */}
                    {remaining > 0 && (
                      <button type="button" className="ghost incidents-show-more" onClick={() => showMore(g.key)}>
                        {t("incidents.group.showMore", { count: remaining })}
                      </button>
                    )}
                    {/* Tope global de DOM alcanzado y este grupo todavía tiene cards sin montar. */}
                    {remaining === 0 && rendered.length < g.cards.length && (
                      <div className="muted incidents-cap-note">{t("incidents.group.capReached", { count: INCIDENT_GROUP_DOM_CAP })}</div>
                    )}
                  </div>
                )}
              </section>
            );
          })}
        </div>
      )}
      {detail && (
        <IncidentSlideOver
          card={detail}
          nowIso={nowIso}
          onClose={() => setDetailCard(null)}
          onDecide={onDecide}
          onGoToSource={onGoToSource}
          onOpenAudit={onOpenAudit}
          // 044 FR-003 — el slide-over también respeta el guard: si la card está en vuelo, sus acciones
          // de decisión quedan deshabilitadas y muestra el error inline (no se puede re-decidir doble).
          deciding={isDeciding?.(detail.id)}
          errorMsg={cardErrors?.[detail.id]}
        />
      )}
    </div>
  );
}

function CardItem({ card, nowIso, focused, onDecide, onGoToSource, onShowDetail, onFocusCard, deciding, errorMsg }: {
  card: Card; nowIso: string; focused?: boolean;
  onDecide: DecideFn; onGoToSource: (c: Card) => void; onShowDetail: () => void; onFocusCard: () => void;
  // 044 FR-003 — decisión en vuelo (deshabilita los botones de decisión) + error inline (la card NO
  // desaparece; muestra el error y deja re-intentar).
  deciding?: boolean; errorMsg?: string;
}) {
  const t = useT();
  const state = inboxState(card, nowIso);
  const target = sourceTarget(card);
  return (
    <article
      className={`card-item sev-${card.severity} state-${state} ${focused ? "is-focused" : ""} ${deciding ? "is-deciding" : ""} ${errorMsg ? "has-error" : ""}`}
      onMouseEnter={onFocusCard}
      tabIndex={0}
      onFocus={onFocusCard}
      aria-busy={deciding || undefined}
    >
      <div className="card-row">
        <span className="card-project">{card.project}</span>
        <span className="card-title">{card.title}</span>
        {card.reopened ? <span className="badge-reopened" title={t("incidents.reopenedHint")}>{t("incidents.reopened")}</span> : null}
        <span className="card-when">{fmtWhen(card.created_at)}</span>
      </div>
      <div className="card-row card-meta">
        <span className="muted">{t("incidents.detail.source")}: {card.source}</span>
        <span className="muted">·</span>
        <span className={`sev-tag sev-${card.severity}`}>{severityLabel(t, card.severity)}</span>
      </div>
      {/* 044 FR-003 — error inline de la última decisión (la card NO desaparece). */}
      {errorMsg && <div className="card-error" role="alert">{errorMsg}</div>}
      <div className="card-actions">
        {/* "detalle"/"ir al origen" NO son decisiones → siguen habilitados durante el invoke. */}
        <button type="button" onClick={onShowDetail}>{t("incidents.action.details")}</button>
        <button
          type="button"
          onClick={() => (target.view ? onGoToSource(card) : onShowDetail())}
          title={t("incidents.action.goToSource")}
        >
          ⤴ {t(target.labelKey)}
        </button>
        {/* Acciones de DECISIÓN: deshabilitadas mientras hay un invoke en vuelo para esta card. */}
        <SnoozeMenu onSnooze={(until) => onDecide(card.id, "snoozed", until)} disabled={deciding} />
        {!card.read_at && <button type="button" disabled={deciding} onClick={() => onDecide(card.id, "read")}>{t("incidents.action.markRead")}</button>}
        <button type="button" disabled={deciding} onClick={() => onDecide(card.id, "dismissed")}>{t("incidents.action.dismiss")}</button>
        {card.severity === "critical" && (
          <button type="button" onClick={() => invoke("telegram_emit_card", { cardId: card.id }).catch(console.error)} title="F23 · Telegram relay (HMAC)">
            📱 TG
          </button>
        )}
        <button type="button" className="primary" disabled={deciding} onClick={() => onDecide(card.id, "approved")}>{t("incidents.action.approve")}</button>
        <button type="button" className="danger" disabled={deciding} onClick={() => onDecide(card.id, "rejected")}>{t("incidents.action.reject")}</button>
      </div>
    </article>
  );
}

function RailCard({ card, nowIso, onDecide, onGoToSource, deciding, errorMsg }: {
  card: Card; nowIso: string; onDecide: DecideFn; onGoToSource: (c: Card) => void;
  // 044 FR-003 — decisión en vuelo (deshabilita decisiones) + error inline (la card NO desaparece).
  deciding?: boolean; errorMsg?: string;
}) {
  const t = useT();
  const target = sourceTarget(card);
  void nowIso; // disponible para estado futuro; el rail siempre muestra cards accionables.
  return (
    <div className={`rail-card sev-${card.severity} ${deciding ? "is-deciding" : ""} ${errorMsg ? "has-error" : ""}`} aria-busy={deciding || undefined}>
      <div className="rail-card-title">
        {card.title}
        {card.reopened ? <span className="badge-reopened" title={t("incidents.reopenedHint")}> {t("incidents.reopened")}</span> : null}
      </div>
      <div className="rail-card-meta"><span className="muted">{card.project}</span> · <span className="muted">{fmtWhen(card.created_at)}</span></div>
      {errorMsg && <div className="card-error" role="alert">{errorMsg}</div>}
      <div className="rail-card-actions">
        <button className="mini primary" disabled={deciding} onClick={() => onDecide(card.id, "approved")} aria-label={t("chrome.rail.cardOkAria")} title={t("chrome.rail.cardOkAria")}>{t("chrome.rail.cardOk")}</button>
        {target.view && (
          <button className="mini" onClick={() => onGoToSource(card)} aria-label={t("incidents.action.goToSource")} title={t(target.labelKey)}>⤴</button>
        )}
        <SnoozeMenu compact onSnooze={(until) => onDecide(card.id, "snoozed", until)} disabled={deciding} />
        <button className="mini danger" disabled={deciding} onClick={() => onDecide(card.id, "rejected")} aria-label={t("chrome.rail.cardRejectAria")} title={t("chrome.rail.cardRejectAria")}>{t("chrome.rail.cardReject")}</button>
      </div>
    </div>
  );
}

/** 022 P0b — un monitor está "caído" si tiene resultado y NO está up. Pura/testeable. */
export function isMonitorDown(m: MonitorSnapshot): boolean {
  return !!m.last && !m.last.up;
}

function MonitorsView({ monitors, initialDownOnly, onClearFilter, onChanged }: {
  monitors: MonitorSnapshot[];
  // 022 P0b — drill-down: si el stat "Monitors X/Y up" nos trajo, arrancar filtrado a caídos.
  initialDownOnly?: boolean;
  onClearFilter?: () => void;
  // 045 FR-001 — callback para refrescar la lista tras agregar/quitar un target.
  onChanged?: () => void;
}) {
  const [downOnly, setDownOnly] = useState<boolean>(!!initialDownOnly);
  useEffect(() => { setDownOnly(!!initialDownOnly); }, [initialDownOnly]);
  // 045 FR-001 — formulario de alta de monitor.
  const [label, setLabel] = useState("");
  const [kind, setKind] = useState<"tcp" | "http">("tcp");
  const [addr, setAddr] = useState("");
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState<string | null>(null);
  const downCount = monitors.filter(isMonitorDown).length;
  const shown = downOnly ? monitors.filter(isMonitorDown) : monitors;

  const add = async () => {
    setErr(null); setBusy(true);
    try {
      await invoke<string>("monitor_add", { label: label.trim(), kind, addr: addr.trim() });
      setLabel(""); setAddr("");
      onChanged?.();
    } catch (e) {
      setErr(String(e));
    } finally { setBusy(false); }
  };
  const remove = async (id: string) => {
    setBusy(true);
    try { await invoke<boolean>("monitor_remove", { id }); onChanged?.(); }
    catch (e) { setErr(String(e)); }
    finally { setBusy(false); }
  };

  return (
    <div className="page">
      <div className="page-header">
        <div className="page-title">Monitors</div>
        <div className="page-sub">poll por target (default 30s) · {downCount} caído(s)</div>
        <div className="page-actions">
          <button
            type="button"
            className={downOnly ? "ghost is-active" : "ghost"}
            aria-pressed={downOnly}
            onClick={() => { const next = !downOnly; setDownOnly(next); if (!next) onClearFilter?.(); }}
          >
            {downOnly ? "Mostrando caídos" : "Filtrar caídos"}
          </button>
        </div>
      </div>

      {/* 045 FR-001 — alta de monitor configurable. */}
      <form
        className="mon-add"
        onSubmit={(e) => { e.preventDefault(); if (!busy && label.trim() && addr.trim()) add(); }}
        style={{ display: "flex", gap: 8, flexWrap: "wrap", alignItems: "center", marginBottom: 12 }}
      >
        <input
          aria-label="Etiqueta del monitor"
          placeholder="Etiqueta (ej. AIE local)"
          value={label}
          onChange={(e) => setLabel(e.target.value)}
          disabled={busy}
        />
        <select aria-label="Tipo de monitor" value={kind} onChange={(e) => setKind(e.target.value as "tcp" | "http")} disabled={busy}>
          <option value="tcp">TCP (host:port)</option>
          <option value="http">HTTP(S) (url)</option>
        </select>
        <input
          aria-label="Dirección del monitor"
          placeholder={kind === "tcp" ? "127.0.0.1:22" : "http://localhost:8250/health"}
          value={addr}
          onChange={(e) => setAddr(e.target.value)}
          disabled={busy}
          style={{ minWidth: 240 }}
        />
        <button type="submit" className="primary" disabled={busy || !label.trim() || !addr.trim()}>
          {busy ? "…" : "Agregar"}
        </button>
      </form>
      {err && <div className="muted" role="alert" style={{ color: "var(--danger, #d33)", marginBottom: 10 }}>{err}</div>}

      {shown.length === 0 && downOnly
        ? <div className="empty"><span className="glyph" /><div className="head">Sin monitores caídos</div><div className="body muted">Todo arriba.</div></div>
        : shown.length === 0
        ? <div className="empty"><span className="glyph" /><div className="head">Sin monitores</div><div className="body muted">Agregá uno arriba.</div></div>
        : <div className="mon-grid">
        {shown.map((m) => (
          <div key={m.target.id} className={`mon ${m.last?.up ? "up" : (m.last ? "down" : "")}`}>
            <div className="mon-head">
              <span className={`dot ${m.last?.up ? "up" : (m.last ? "down" : "unknown")}`} />
              <span className="mon-label">{m.target.label}</span>
              <span className="mon-addr muted">{m.target.addr}</span>
              <button
                type="button"
                className="ghost"
                aria-label={`Quitar ${m.target.label}`}
                title="Quitar monitor"
                disabled={busy}
                onClick={() => remove(m.target.id)}
                style={{ marginLeft: "auto" }}
              >×</button>
            </div>
            <div className="mon-body">
              {m.last ? (
                m.last.up
                  ? <span>up · <strong>{m.last.latency_ms}ms</strong></span>
                  : <span className="muted">down · {m.last.error}</span>
              ) : <span className="muted">checking…</span>}
              {m.last && <span className="muted" style={{ marginLeft: 12 }}>· {fmtTime(m.last.checked_at)}</span>}
            </div>
          </div>
        ))}
      </div>}
    </div>
  );
}

function AuditView({ events }: { events: AuditEvent[] }) {
  return (
    <div className="page">
      <div className="page-header"><div className="page-title">Audit log</div><div className="page-sub">append-only · últimos 100 eventos</div></div>
      {events.length === 0
        ? <div className="empty"><span className="glyph" /><div className="head">No events</div></div>
        : <div className="audit-list">{events.map((e) => (
            <div key={e.id} className="audit-row">
              <span className="at">{fmtTime(e.at)}</span>
              <span className="k">{e.kind}</span>
              <span className="p">{e.actor}</span>
            </div>
          ))}</div>}
    </div>
  );
}

function Placeholder({ title, sub }: { title: string; sub: string }) {
  return (
    <div className="page">
      <div className="page-header"><div className="page-title">{title}</div><div className="page-sub">{sub}</div></div>
      <div className="empty"><span className="glyph" /><div className="head">Pendiente</div><div className="body muted">{sub}</div></div>
    </div>
  );
}
