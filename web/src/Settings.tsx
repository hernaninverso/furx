import { useEffect, useState } from "react";
import { FeatureFlagsPanel } from "./components/FeatureFlagsPanel"; // 015 T022
import { LanguageSwitch } from "./components/LanguageSwitch"; // 016 US1 — selector de idioma
import { useT } from "./lib/i18n"; // 016 US1 — t() para el subset de alto tráfico
import { DEFAULT_PTT_HOTKEY, parsePttHotkey, formatPttHotkey, eventToHotkeyString, setPttCapturing } from "./lib/pttHotkey"; // 059 — rebind PTT
import { invalidateTelemetryConfig } from "./lib/telemetry"; // 016 US5 M1 — opt-out inmediato
import { invoke } from "./lib/invoke"; // 015 T015: invoke con flujo de aprobación universal
import { ConnectStatusPanel } from "./components/ConnectStatusPanel";
import { CloudAccountPanel } from "./components/CloudAccountPanel";
import { SignalsPanel } from "./components/SignalsPanel";
import { ConnectScreen } from "./wizard/ConnectScreen";
import { LegalModal } from "./components/LegalModal";
import { MobilePairingQR } from "./components/MobilePairingQR"; // 065 — pareo por QR
import { EULA, PRIVACY_POLICY, TERMS_OF_SERVICE, DPA_TEMPLATE, APACHE_LICENSE, OSS_NOTICES, LEGAL_VERSION } from "./legal";
import { Button } from "./components/Button";

interface Compat {
  macos_ok: boolean; macos_version: string;
  arch_ok: boolean; arch: string;
  claude_cli: string | null; codex_cli: string | null; gemini_cli: string | null; aider_cli: string | null;
  grok_cli: string | null; // 062
  tmux: string | null; git: string | null;
  all_ok: boolean;
}

interface UpdateInfo {
  current: string; latest: string | null; url: string | null; error: string | null;
}

// Backend export.rs::ExportReport — devuelto por export_state_to_desktop / export_state.
interface ExportReport {
  path: string; size_bytes: number; sha256: string; items: string[]; filtered: string[];
}

interface MobileStatus {
  running: boolean; addrs: string[]; tailscale_ip: string | null;
  loopback_port: number; tailscale_port: number;
}

// 022 US7 / FR-012 — pestaña inicial con la que abrimos `ConnectScreen` desde Ajustes. Subconjunto
// del `TabKey` interno del wizard; sólo necesitamos apuntar a la gestión de cuentas de CLI.
type ConnectTab = "claude-accounts";

export function SettingsView() {
  const t = useT(); // 016 US1 — copy de alto tráfico vía i18n (subset; el resto cae al fuente).
  const [settings, setSettings] = useState<Array<[string, unknown]>>([]);
  const [compat, setCompat] = useState<Compat | null>(null);
  const [update, setUpdate] = useState<UpdateInfo | null>(null);
  const [busy, setBusy] = useState(false);
  const [msg, setMsg] = useState<string | null>(null);
  const [connectModalOpen, setConnectModalOpen] = useState(false);
  // 022 US7 / FR-012 — pestaña inicial del wizard al abrirlo desde Ajustes (ej. "claude-accounts"
  // para ir directo a la gestión de cuentas, sin pasar por el onboarding/ProGate).
  const [connectInitialTab, setConnectInitialTab] = useState<ConnectTab | undefined>(undefined);
  const openConnect = (initialTab?: ConnectTab) => { setConnectInitialTab(initialTab); setConnectModalOpen(true); };
  // La galería de agentes (perfiles, spec 006) vive en Shell (estado `agentGalleryOpen`). Desde Ajustes
  // la abrimos por el bus de eventos `furx:*` que la app ya usa (Shell escucha y setea su estado), para
  // NO acoplar Settings con la chrome ni romper las otras entradas (command palette / panes la abren igual).
  const openAgentGallery = () => { window.dispatchEvent(new CustomEvent("furx:open-agents")); };
  const [mobileSecret, setMobileSecret] = useState<string | null>(null);
  const [mobileStatus, setMobileStatus] = useState<MobileStatus | null>(null);
  const [secretShown, setSecretShown] = useState(false);

  const refresh = async () => {
    const [s, c] = await Promise.all([
      invoke<Array<[string, unknown]>>("settings_all"),
      invoke<Compat>("compat_check"),
    ]);
    setSettings(s); setCompat(c);
  };

  // 015 T015 (audit codex MED): NO auto-cargar el SECRET — `mobile_secret_get` es Credential
  // (gateado) y auto-fetchearlo en el mount dispararía el modal de aprobación al abrir Settings.
  // Cargamos sólo el STATUS (Safe) automáticamente; el secret se revela bajo un botón explícito
  // (`revealMobileSecret`), que SÍ pasa por el gate → aprobación en el click (BYOK: el secret se
  // muestra sólo con acción deliberada del usuario).
  const loadMobile = async () => {
    try {
      const st = await invoke<MobileStatus>("mobile_bridge_status");
      setMobileStatus(st);
    } catch { /* bridge not started yet — non-fatal */ }
  };

  // Revela el secret de pareo móvil: fetch explícito (Credential → aprobación en el gate).
  const revealMobileSecret = async () => {
    try {
      const sec = await invoke<string>("mobile_secret_get");
      setMobileSecret(sec); setSecretShown(true);
    } catch { /* aprobación rechazada o bridge no iniciado — non-fatal */ }
  };

  const rotateMobileSecret = async () => {
    if (!confirm("Rotar el secret invalida los teléfonos ya pareados y requiere reiniciar Furx para aplicar.\n\n¿Seguro?")) return;
    setBusy(true); setMsg(null);
    try {
      const s = await invoke<string>("mobile_secret_rotate");
      setMobileSecret(s); setSecretShown(true);
      setMsg("secret rotado — reiniciá Furx para que el bridge use el nuevo");
    } catch (e) { setMsg(`error: ${String(e)}`); }
    finally { setBusy(false); }
  };

  useEffect(() => { refresh(); loadMobile(); }, []);

  const setKey = async (key: string, value: unknown) => {
    setBusy(true); setMsg(null);
    try {
      await invoke("settings_set", { key, value });
      await refresh();
      setMsg(`saved ${key}`);
    } catch (e) { setMsg(`error: ${String(e)}`); }
    finally { setBusy(false); }
  };

  const checkUpdates = async () => {
    setBusy(true); setMsg(null);
    try {
      const u = await invoke<UpdateInfo>("check_updates");
      setUpdate(u);
    } catch (e) { setMsg(`error: ${String(e)}`); }
    finally { setBusy(false); }
  };

  const reset = async (level: "soft" | "hard" | "full") => {
    const confirms: Record<string, string> = {
      "soft": "Borrar WAL/SHM (mantiene furx.db).",
      "hard": "Borrar TODO ~/.furx (data + settings).",
      "full": "Hard reset + recordatorio Keychain.",
    };
    if (!confirm(`${level.toUpperCase()} RESET\n\n${confirms[level]}\n\n¿Seguro?`)) return;
    setBusy(true);
    try {
      const r = await invoke<{ level: string; removed: string[] }>("reset_furx", { level });
      setMsg(`reset ${r.level}: ${r.removed.length} items`);
    } catch (e) { setMsg(`error: ${String(e)}`); }
    finally { setBusy(false); }
  };

  const get = (key: string): unknown => settings.find(([k]) => k === key)?.[1];

  const sections: { id: string; label: string }[] = [
    { id: "connect", label: "Conexión" },
    { id: "accounts", label: t("settings.accounts.section") }, // 022 US7 / FR-012
    { id: "cloud", label: "Cuenta cloud" },
    { id: "sync", label: "Sincronización" }, // 050 FR-001 — multi-machine sync
    { id: "endpoints", label: "Servicios" },
    { id: "privacy", label: "Privacidad" },
    { id: "mobile", label: "Móvil" },
    { id: "integrations", label: "Integraciones" },
    { id: "compat", label: "Sistema" },
    { id: "screens", label: "Pantallas" }, // 053 — monitores físicos
    { id: "license", label: "Licencia" }, // 053 — install ID
    { id: "speckit", label: "Spec-kit" }, // 053 — alias status
    { id: "sync-manual", label: "Sync manual" }, // 053 — sync_run
    { id: "preferences", label: "Preferencias" }, // 053 — preference_records_list
    { id: "audio-attention", label: "Audio · Atención" }, // 053 — opt-in por pane
    { id: "shortcuts", label: "Atajos" }, // 059 — rebind del hotkey de push-to-talk
    { id: "provider-lookup", label: "Proveedores" }, // 053 — provider_get
    { id: "updates", label: "Actualizaciones" },
    { id: "data", label: "Datos" },
    { id: "advanced", label: "Avanzado" },
    { id: "legal", label: "Legal" },
    { id: "about", label: "Acerca de" },
  ];

  type LegalDoc = "eula" | "privacy" | "terms" | "dpa" | "apache" | "oss";
  const [legalOpen, setLegalOpen] = useState<LegalDoc | null>(null);
  const legalDocs: Record<LegalDoc, { title: string; body: string }> = {
    eula:    { title: `EULA · v${LEGAL_VERSION}`, body: EULA },
    privacy: { title: `Política de Privacidad · v${LEGAL_VERSION}`, body: PRIVACY_POLICY },
    terms:   { title: `Términos de Servicio · v${LEGAL_VERSION}`, body: TERMS_OF_SERVICE },
    dpa:     { title: `DPA (plantilla Compliance Pack) · v${LEGAL_VERSION}`, body: DPA_TEMPLATE },
    apache:  { title: "Licencia Apache-2.0", body: APACHE_LICENSE },
    oss:     { title: "Componentes open source incluidos", body: OSS_NOTICES },
  };

  return (
    <div className="page settings-page">
      <div className="settings-layout">
        <aside className="settings-nav">
          <div className="settings-nav-title">Ajustes</div>
          {sections.map((s) => (
            <a key={s.id} href={`#${s.id}`} className="settings-nav-link">{s.label}</a>
          ))}
        </aside>
        <div className="settings-content">
          {msg && <div className="toast-inline">{msg}</div>}

          <Section id="connect" title="Conexión" hint="Tus proveedores LLM (Bring Your Own Keys)">
            <ConnectStatusPanel onOpenConnect={() => openConnect()} />
          </Section>

          {/* 022 US7 / FR-012 — entrada a Cuentas y Perfiles desde Ajustes (antes sólo Wizard/ProGate). */}
          <Section id="accounts" title={t("settings.accounts.section")} hint={t("settings.accounts.hint")}>
            <div className="actions-row" style={{ flexWrap: "wrap", gap: 12 }}>
              <button
                type="button"
                className="legal-link"
                onClick={() => openConnect("claude-accounts")}
                aria-label={t("settings.accounts.manage")}
              >
                <strong>{t("settings.accounts.manage")}</strong>
                <span className="muted">{t("settings.accounts.manageHint")}</span>
              </button>
              <button
                type="button"
                className="legal-link"
                onClick={openAgentGallery}
                aria-label={t("settings.accounts.gallery")}
              >
                <strong>{t("settings.accounts.gallery")}</strong>
                <span className="muted">{t("settings.accounts.galleryHint")}</span>
              </button>
            </div>
          </Section>

          <Section id="cloud" title="Cuenta cloud" hint="Sign-in opcional para sincronización + traces + replay (Pro). Tu Furx funciona offline sin esto.">
            <CloudAccountPanel />
          </Section>

          {/* 050 FR-001 — sincronización multi-máquina (opt-in, fail-closed). */}
          <Section id="sync" title="Sincronización multi-máquina" hint="Sincroniza tus overrides MCP, monitores y gotchas entre tus máquinas (mismo usuario cloud). Opt-in, desactivado por defecto. Si el relay no responde, cada máquina sigue con su estado local.">
            <MultiSyncPanel get={get} setKey={setKey} />
          </Section>

          <Section id="endpoints" title="Servicios" hint="Endpoints opcionales: sincronización, licencia, telemetría">
            <Row label="Motor IA (backend del Council)" value={get("endpoints.aie") as string} onSave={(v) => setKey("endpoints.aie", v)} placeholder="https://api.tu-proveedor.com" />
            <Row label="Licencia (validación de Pro)" value={get("endpoints.license") as string} onSave={(v) => setKey("endpoints.license", v)} placeholder="https://tu-licencia.example.com" />
            <Row label="Telemetría (opcional)" value={get("endpoints.telemetry") as string} onSave={(v) => { setKey("endpoints.telemetry", v); invalidateTelemetryConfig(); }} placeholder="dejar vacío para deshabilitar" />
            <Row label="Actualizaciones (feed de releases)" value={get("endpoints.updates") as string} onSave={(v) => setKey("endpoints.updates", v)} placeholder="https://github.com/owner/repo/releases/latest/download/latest.json" />
          </Section>

          <Section id="privacy" title="Privacidad" hint="Tus datos nunca salen del Mac sin tu permiso explícito">
            <Toggle label={t("settings.telemetry.label")} hint={t("settings.telemetry.hint")} value={get("opt_in.telemetry") === true} onChange={(v) => { setKey("opt_in.telemetry", v); invalidateTelemetryConfig(); }} />
            <Toggle label="Reportes de fallos" hint="Stacktraces locales cuando la app falla. Sin datos sensibles." value={get("opt_in.crash_reports") === true} onChange={(v) => setKey("opt_in.crash_reports", v)} />
            <Toggle label="Preguntar antes de restaurar al abrir" hint="Por defecto, Furx reattachea tus sesiones tmux silenciosamente. Activá esto si preferís elegir cada vez." value={get("restore.always_ask") === true} onChange={(v) => setKey("restore.always_ask", v)} />
            <div className="row-meta"><span className="muted">Términos aceptados el:</span> <code>{formatTs(get("opt_in.eula_accepted_at"))}</code></div>
          </Section>

          <Section id="audio" title="Audio de avisos" hint="Voz, velocidad y volumen de los avisos de la cola de atención (TTS + earcon). Default = sistema.">
            <AudioPrefsPanel />
          </Section>

          <Section id="shortcuts" title="Atajos" hint="Reasigná el atajo de push-to-talk (mantener para grabar voz). Default ⌥Space.">
            <PttHotkeyRow
              current={(get("ptt.hotkey") as string) || DEFAULT_PTT_HOTKEY}
              onSave={async (combo) => {
                // 059 (audit r1+r2): invoke DIRECTO (no `setKey`, que traga el error y resuelve igual).
                // Sólo si el PERSIST tiene éxito despachamos el update EN VIVO — e INMEDIATAMENTE después
                // del persist, ANTES de `refresh()` (r2: si refresh fallara, vivo y persistido quedarían
                // desincronizados al revés). `refresh()` (sólo refresca el display de Ajustes) va aparte,
                // best-effort: su fallo no revierte el estado ya consistente (persistido == en vivo).
                try {
                  await invoke("settings_set", { key: "ptt.hotkey", value: combo });
                } catch (e) {
                  setMsg(`error: ${String(e)}`);
                  return;
                }
                window.dispatchEvent(new CustomEvent("furx:ptt-hotkey", { detail: combo }));
                setMsg("saved ptt.hotkey");
                try { await refresh(); } catch { /* el valor ya persistió y está en vivo */ }
              }}
            />
          </Section>

          <Section id="mobile" title="Móvil" hint="Companion iOS/Android — ver panes, enviar comandos por voz/texto, aprobar tool-calls y recibir notificaciones. Sin claves de proveedor en el teléfono (BYOK intacto).">
            {(() => {
              const port = mobileStatus?.loopback_port ?? 43118;
              const tsPort = mobileStatus?.tailscale_port ?? 43119;
              const host = mobileStatus?.tailscale_ip ? `${mobileStatus.tailscale_ip}:${tsPort}` : `127.0.0.1:${port}`;
              const url = `http://${host}/`;
              return (
                <>
                  {/* 065 — pareo por QR (método primario): escaneás desde el companion y queda vinculado. */}
                  <MobilePairingQR />
                  <details style={{ marginBottom: 10 }}>
                    <summary className="muted" style={{ cursor: "pointer" }}>Pareo manual (URL + secret)</summary>
                    <div className="muted" style={{ margin: "8px 0" }}>
                      En el teléfono (por Tailscale; o loopback en el mismo host), abrí esta URL y pegá el secret.
                      El bridge solo escucha en loopback + Tailscale — nunca expuesto a la LAN.
                    </div>
                  <div className="row-meta">
                    <span className="muted">URL:</span> <code>{url}</code>{" "}
                    <button onClick={() => navigator.clipboard?.writeText(url)}>Copiar</button>
                  </div>
                  <div className="row-meta" style={{ marginTop: 8 }}>
                    <span className="muted">Secret:</span>{" "}
                    <code>{secretShown ? (mobileSecret ?? "…") : "•".repeat(16)}</code>{" "}
                    <button onClick={() => { if (secretShown) { setSecretShown(false); } else if (mobileSecret) { setSecretShown(true); } else { void revealMobileSecret(); } }}>{secretShown ? "Ocultar" : "Mostrar"}</button>{" "}
                    <button onClick={() => mobileSecret && navigator.clipboard?.writeText(mobileSecret)} disabled={!mobileSecret}>Copiar</button>{" "}
                    <Button variant="danger" onClick={rotateMobileSecret} disabled={busy}>Rotar</Button>
                  </div>
                  </details>
                  <div className="row-meta" style={{ marginTop: 8 }}>
                    <span className="muted">Bridge:</span>{" "}
                    {mobileStatus?.running
                      ? <code>escuchando · {mobileStatus.addrs.join(", ")}</code>
                      : <span className="muted">no iniciado (reiniciá Furx)</span>}
                  </div>
                  <Toggle
                    label="Acceso por Tailscale"
                    hint={`Permite parear fuera de la LAN por la tailnet (puerto ${tsPort}). Requiere reiniciar Furx.${mobileStatus?.tailscale_ip ? " IP detectada: " + mobileStatus.tailscale_ip : " — sin IP Tailscale detectada"}`}
                    value={get("mobile.tailscale_enabled") === true}
                    onChange={(v) => setKey("mobile.tailscale_enabled", v)}
                  />
                  <h4 style={{ marginTop: 20, marginBottom: 8, fontSize: 13, color: "var(--text)" }}>Notificaciones al teléfono</h4>
                  <Toggle label="Cards de Furx" hint="Algo requiere tu atención (tool-call pendiente, error)." value={get("mobile.notify.card") !== false} onChange={(v) => setKey("mobile.notify.card", v)} />
                  <Toggle label="Alertas de Grafana" hint={`Webhook: POST a http://${host}/furx/v1/grafana con header Authorization: Bearer <secret>.`} value={get("mobile.notify.grafana") !== false} onChange={(v) => setKey("mobile.notify.grafana", v)} />
                  <Toggle label="Pane listo (Claude esperando input)" hint="Aviso cuando un pane pasa de ocupado a listo." value={get("mobile.notify.pane_ready") !== false} onChange={(v) => setKey("mobile.notify.pane_ready", v)} />
                  <Toggle label="Eventos del Council (auditoría)" hint="Opt-in. Avisos de corridas del Council. Por defecto: deshabilitado." value={get("mobile.notify.audit") === true} onChange={(v) => setKey("mobile.notify.audit", v)} />
                </>
              );
            })()}
          </Section>

          <Section id="integrations" title="Integraciones" hint="Notificaciones multi-canal + control remoto por Telegram. Tokens en el Keychain (BYOK), nunca en un backend.">
            <Row label="Relay de Telegram (endpoint)" value={get("endpoints.telegram_relay") as string} onSave={(v) => setKey("endpoints.telegram_relay", v)} placeholder="https://tu-relay.example.com/furx" />
            <SignalsPanel get={(k) => get(k)} setKey={(k, v) => setKey(k, v)} setMsg={setMsg} />
          </Section>

          <Section id="compat" title="Sistema" hint="Detección de herramientas y compatibilidad">
            {compat ? (
              <div className="compat-grid">
                <Item ok={compat.macos_ok} label={`macOS ${compat.macos_version}`} req="≥ 11" />
                <Item ok={compat.arch_ok} label={`Arquitectura: ${compat.arch}`} req="aarch64" />
                <Item ok={!!compat.tmux} label="tmux" req={compat.tmux ?? "no instalado"} install={!compat.tmux ? "brew install tmux" : null} />
                <Item ok={!!compat.git} label="git" req={compat.git ?? "no instalado"} install={!compat.git ? "xcode-select --install" : null} />
                <Item ok={!!compat.claude_cli} label="Claude Code CLI" req={compat.claude_cli ?? "opcional"} install={!compat.claude_cli ? "https://docs.anthropic.com/claude-code" : null} />
                <Item ok={!!compat.codex_cli} label="Codex CLI" req={compat.codex_cli ?? "opcional"} install={!compat.codex_cli ? "npm i -g @openai/codex-cli" : null} />
                <Item ok={!!compat.gemini_cli} label="Gemini CLI" req={compat.gemini_cli ?? "opcional"} install={!compat.gemini_cli ? "npm i -g @google/gemini-cli" : null} />
                <Item ok={!!compat.aider_cli} label="Aider" req={compat.aider_cli ?? "opcional"} install={!compat.aider_cli ? "pip install aider-chat" : null} />
                <Item ok={!!compat.grok_cli} label="Grok CLI" req={compat.grok_cli ?? "opcional"} install={!compat.grok_cli ? "https://x.ai/grok" : null} />
              </div>
            ) : <div className="muted">verificando…</div>}
          </Section>

          {/* 053 — monitores físicos */}
          <Section id="screens" title="Pantallas" hint="Monitores físicos detectados por el sistema.">
            <ScreensPanel />
          </Section>

          {/* 053 — install ID de la licencia */}
          <Section id="license" title="Licencia" hint="Identificador único de instalación (UUID). Úsalo para vincular tu licencia Pro.">
            <LicensePanel />
          </Section>

          {/* 053 — estado del alias spec-kit */}
          <Section id="speckit" title="Spec-kit" hint="Alias de línea de comandos para el flujo spec-driven de desarrollo (specify CLI).">
            <SpeckitPanel />
          </Section>

          {/* 053 — sync_run manual con remote opcional */}
          <Section id="sync-manual" title="Sync manual" hint="Dispara un snapshot+commit del estado local (overrides MCP, gotchas, config). Remote opcional (git URL); si se omite usa el remote por defecto.">
            <SyncManualPanel />
          </Section>

          {/* 053 — tabla de preferencias grabadas (preference_records_list) */}
          <Section id="preferences" title="Preferencias aprendidas" hint="Registros de preferencia derivados de reviews best-of-N. Lectura: últimas 20 entradas.">
            <PreferencesPanel />
          </Section>

          {/* 053 — opt-in de audio por pane (attention_audio_opt_in_get/set) */}
          <Section id="audio-attention" title="Audio · Atención" hint="Activa o desactiva el aviso de audio de la cola de atención para un pane específico.">
            <AudioOptInPanel />
          </Section>

          {/* 053 — búsqueda de provider por alias (provider_get) */}
          <Section id="provider-lookup" title="Proveedores · búsqueda" hint="Consulta la credencial de un proveedor por alias. Útil para depurar un proveedor específico.">
            <ProviderLookupPanel />
          </Section>

          <Section id="updates" title="Actualizaciones">
            <button onClick={checkUpdates} disabled={busy}>Buscar actualizaciones</button>
            {update && (
              <div className="row-meta" style={{ marginTop: 12 }}>
                actual <code>{update.current}</code> · última <code>{update.latest ?? "n/a"}</code>
                {update.url && /^https:\/\//i.test(update.url) && <> · <a href={update.url} target="_blank" rel="noreferrer">release notes</a></>}
                {update.error && <span className="muted"> · err: {update.error}</span>}
              </div>
            )}
          </Section>

          <Section id="data" title="Datos" hint="Export, import, reset">
            <div className="muted" style={{ marginBottom: 10 }}>
              Empaqueta tu config local (<code>~/.furx/furx.db</code>) en un archivo portable
              <code>.furxexport</code>. El guardrail de secretos filtra cualquier valor sensible antes.
            </div>
            <button onClick={doExport} disabled={busy}>Exportar estado</button>

            <h4 style={{ marginTop: 20, marginBottom: 8, fontSize: 13, color: "var(--text)" }}>Reset</h4>
            <div className="muted" style={{ marginBottom: 10 }}>
              Antes de hard/full, exportá tu estado.
            </div>
            <div className="actions-row">
              <button onClick={() => reset("soft")} disabled={busy}>Soft reset</button>
              <button onClick={() => reset("hard")} disabled={busy}>Hard reset</button>
              <Button variant="danger" onClick={() => reset("full")} disabled={busy}>Full uninstall</Button>
            </div>
          </Section>

          <Section id="advanced" title="Avanzado" hint="Idioma · tmux watchdog · feature flags">
            {/* 016 US1 — selector de idioma de la interfaz. */}
            <LanguageSwitch />
            <TmuxWatchdogPanel busy={busy} setBusy={setBusy} setMsg={setMsg} />
            {/* 015 T022 — feature flags locales. */}
            <div style={{ marginTop: 16 }}>
              <h4 style={{ margin: "0 0 6px" }}>Características experimentales</h4>
              <FeatureFlagsPanel />
            </div>
            {/* 036 — Motor de decisión LOCAL (offline) para el meta-orquestador. */}
            <div style={{ marginTop: 16 }}>
              <h4 style={{ margin: "0 0 6px" }}>Orquestación · motor local (offline)</h4>
              <Toggle
                label="Motor local (offline)"
                hint="Refina la detección de tareas terminadas con un modelo LOCAL (Ollama) en vez del servicio en la nube. No necesita conexión ni cuenta. Es advisory: si el modelo no responde, Furx usa su heurística de siempre. Por defecto: deshabilitado."
                value={get("orchestration.meta_decision.local_engine") === true}
                onChange={(v) => setKey("orchestration.meta_decision.local_engine", v)}
              />
              <Row
                label="Endpoint Ollama"
                value={(get("meta_decision.ollama_endpoint") as string) ?? ""}
                placeholder="http://127.0.0.1:11434"
                onSave={(raw) => {
                  // UX: clamp a loopback. El gate DURO está en el backend (FR-007): un endpoint
                  // no-loopback igual se rechaza al consultar (degrada a la heurística, sin red).
                  const v = raw.trim() || "http://127.0.0.1:11434";
                  if (!isLoopbackUrl(v)) {
                    setMsg("El endpoint debe ser loopback (127.0.0.1 / localhost). Furx solo consulta el modelo local en tu propia máquina.");
                    return;
                  }
                  setKey("meta_decision.ollama_endpoint", v);
                }}
              />
              <Row
                label="Modelo Ollama"
                value={(get("meta_decision.ollama_model") as string) ?? ""}
                placeholder="qwen2.5:3b"
                onSave={(raw) => setKey("meta_decision.ollama_model", raw.trim() || "qwen2.5:3b")}
              />
            </div>
          </Section>

          <Section id="legal" title="Legal" hint="Documentos completos · todos accesibles, todos versionados">
            <div style={{ display: "grid", gridTemplateColumns: "repeat(auto-fit, minmax(220px, 1fr))", gap: 8 }}>
              <button className="legal-link" onClick={() => setLegalOpen("eula")}>
                <strong>EULA</strong>
                <span className="muted">Acuerdo de uso final</span>
              </button>
              <button className="legal-link" onClick={() => setLegalOpen("privacy")}>
                <strong>Privacidad</strong>
                <span className="muted">Qué guarda Furx y qué sale del Mac</span>
              </button>
              <button className="legal-link" onClick={() => setLegalOpen("terms")}>
                <strong>Términos de servicio</strong>
                <span className="muted">Suscripción Pro, cancelación, SLA</span>
              </button>
              <button className="legal-link" onClick={() => setLegalOpen("dpa")}>
                <strong>DPA (plantilla)</strong>
                <span className="muted">Compliance Pack · para Enterprise</span>
              </button>
            </div>
            <p className="muted" style={{ marginTop: 12, fontSize: 12 }}>
              Versión vigente: <code>{LEGAL_VERSION}</code> ·
              {get("opt_in.eula_accepted_at")
                ? <> aceptados el <code>{formatTs(get("opt_in.eula_accepted_at"))}</code></>
                : <> sin aceptación registrada — el wizard te lo va a pedir en el próximo inicio</>}
            </p>
          </Section>

          <Section id="about" title="Acerca de Furx">
            <div className="row-meta">
              <span className="muted">Furx — Run any coding agent side-by-side. No proxy. ·</span>{" "}
              <a href="https://github.com/hernaninverso/furx" target="_blank" rel="noreferrer">github.com/hernaninverso/furx</a>
            </div>
            <div style={{ display: "flex", gap: 8, marginTop: 8 }}>
              <button onClick={() => setLegalOpen("apache")}>Ver licencia Apache-2.0</button>
              <button onClick={() => setLegalOpen("oss")}>Componentes open source</button>
            </div>
          </Section>
        </div>
      </div>

      {connectModalOpen && (
        <ConnectScreen
          initialTab={connectInitialTab}
          onDone={() => { setConnectModalOpen(false); setConnectInitialTab(undefined); refresh(); }}
        />
      )}
      {legalOpen && (
        <LegalModal
          title={legalDocs[legalOpen].title}
          body={legalDocs[legalOpen].body}
          onClose={() => setLegalOpen(null)}
        />
      )}
    </div>
  );

  function doExport() {
    setBusy(true); setMsg(null);
    (async () => {
      try {
        // export_state_to_desktop devuelve un ExportReport (objeto), no un string.
        // Antes se tipaba como <string> y se mostraba directo → "[object Object]".
        const report = await invoke<ExportReport>("export_state_to_desktop").catch(() => null);
        if (!report) {
          // fallback to old command with relative path
          const r = await invoke<ExportReport>(
            "export_state", { outPath: `~/Desktop/furx-${new Date().toISOString().slice(0, 19).replace(/[:T]/g, "-")}.furxexport` }
          );
          setMsg(`Exportado a: ${r.path} · ${(r.size_bytes / 1024).toFixed(1)} KB · ${r.filtered.length} secretos filtrados`);
        } else {
          setMsg(`Exportado a: ${report.path}`);
        }
      } catch (e) { setMsg(`error: ${String(e)}`); }
      finally { setBusy(false); }
    })();
  }
}

function formatTs(v: unknown): string {
  if (typeof v !== "string" || !v) return "pendiente";
  try {
    const d = new Date(v);
    return d.toLocaleDateString();
  } catch {
    return v;
  }
}

function Section({ id, title, hint, children }: { id?: string; title: string; hint?: string; children: React.ReactNode }) {
  return (
    <section id={id} className="settings-section">
      <div className="settings-section-head">
        <h3 className="section-title">{title}</h3>
        {hint && <span className="section-hint">{hint}</span>}
      </div>
      {children}
    </section>
  );
}

// 036 — validación UX de loopback para el endpoint del motor local. El gate DURO (anti-SSRF) vive
// en el backend (`loopback_allowed`, FR-007); esto sólo evita que el usuario tipee un host remoto
// por error. Acepta 127.0.0.0/8, ::1 y localhost (cualquier puerto); rechaza userinfo (`@host`).
function isLoopbackUrl(raw: string): boolean {
  let u: URL;
  try { u = new URL(raw); } catch { return false; }
  if (u.protocol !== "http:" && u.protocol !== "https:") return false;
  // URL.hostname descarta el userinfo (lo que va antes del `@`) → no hay bypass por `127.0.0.1@evil`.
  const h = u.hostname.toLowerCase();
  if (h === "localhost") return true;
  if (h === "[::1]" || h === "::1") return true;
  // 127.0.0.0/8 (cualquier 127.x.x.x).
  const m = h.match(/^(\d{1,3})\.(\d{1,3})\.(\d{1,3})\.(\d{1,3})$/);
  if (m) {
    const oct = m.slice(1).map(Number);
    if (oct.some((o) => o > 255)) return false;
    return oct[0] === 127;
  }
  return false;
}

function Row({ label, value, onSave, placeholder }: { label: string; value?: string; onSave: (v: string) => void; placeholder?: string }) {
  const [v, setV] = useState(value ?? "");
  const [saved, setSaved] = useState(false);
  useEffect(() => { setV(value ?? ""); }, [value]);
  const handleSave = () => {
    onSave(v);
    setSaved(true);
    setTimeout(() => setSaved(false), 1500);
  };
  return (
    <div className="form-row">
      <label>{label}</label>
      <div className="form-input">
        <input value={v} onChange={(e) => setV(e.target.value)} placeholder={placeholder} />
        <button onClick={handleSave} disabled={v === (value ?? "")}>
          {saved ? "✓" : "Guardar"}
        </button>
      </div>
    </div>
  );
}

function Toggle({ label, hint, value, onChange }: { label: string; hint: string; value: boolean; onChange: (v: boolean) => void }) {
  return (
    <div className="form-row toggle-row">
      <label>
        <input type="checkbox" checked={value} onChange={(e) => onChange(e.target.checked)} />{" "}{label}
      </label>
      <div className="hint muted">{hint}</div>
    </div>
  );
}

// 050 FR-001 — panel de sincronización multi-máquina (opt-in, fail-closed). El toggle persiste el
// setting `sync.multi_machine_enabled` (default OFF). "Sincronizar ahora" corre un ciclo; si el opt-in
// está OFF o el relay no responde, NO toca el estado local (lo reporta en el mensaje). Decisión LWW
// `(updated_at, installation_id)` la hace el backend; acá solo disparamos y mostramos el resultado.
interface SyncStatus { enabled: boolean; signed_in: boolean }
interface SyncRunResult { ran: boolean; upserted: number; deleted: number; note: string }
function MultiSyncPanel({ get, setKey }: { get: (k: string) => unknown; setKey: (k: string, v: unknown) => void }) {
  const enabled = get("sync.multi_machine_enabled") === true;
  const [status, setStatus] = useState<SyncStatus | null>(null);
  const [busy, setBusy] = useState(false);
  const [result, setResult] = useState<SyncRunResult | null>(null);

  const refresh = async () => {
    try { setStatus(await invoke<SyncStatus>("sync_status")); } catch { /* sin estado */ }
  };
  useEffect(() => { refresh(); }, []);

  const syncNow = async () => {
    setBusy(true);
    setResult(null);
    try {
      setResult(await invoke<SyncRunResult>("sync_now"));
    } catch (e) {
      setResult({ ran: false, upserted: 0, deleted: 0, note: String(e) });
    } finally {
      setBusy(false);
      refresh();
    }
  };

  return (
    <>
      <Toggle
        label="Activar sincronización multi-máquina"
        hint="Opt-in. Sincroniza overrides MCP, monitores y gotchas entre tus máquinas vía el relay del cloud. Last-write-wins por (timestamp, máquina). Si el relay falla, tu estado local queda intacto."
        value={enabled}
        onChange={(v) => { setKey("sync.multi_machine_enabled", v); }}
      />
      <div className="row-meta" style={{ marginTop: 8 }}>
        <button onClick={syncNow} disabled={busy || !enabled || !(status?.signed_in ?? false)}>
          {busy ? "Sincronizando…" : "Sincronizar ahora"}
        </button>
        {!status?.signed_in && (
          <span className="muted" style={{ marginLeft: 8 }}>Requiere sesión cloud (sección Cuenta cloud).</span>
        )}
      </div>
      {result && (
        <div className="row-meta" style={{ marginTop: 6 }}>
          <span className="muted">{result.note}</span>
          {result.ran && (
            <code style={{ marginLeft: 8 }}>+{result.upserted} aplicados · {result.deleted} borrados</code>
          )}
        </div>
      )}
    </>
  );
}

// 033 U2 — config fina del audio de avisos de la cola de atención (voz/velocidad/volumen del earcon).
// El backend valida/clampa al leer (read_audio_prefs); acá sólo persistimos. GOTCHA Tauri: el getter
// (serde) devuelve snake_case (earcon_volume); los args del setter van en camelCase (earconVolume).
interface AudioPrefs { voice: string | null; rate: number; earcon_volume: number; earcon_sound: string }
function AudioPrefsPanel() {
  const [prefs, setPrefs] = useState<AudioPrefs | null>(null);
  // Valores en vivo de los sliders (muestran el arrastre); se PERSISTEN al soltar (commit), no por
  // cada tick — evita ráfaga de escrituras IPC (audit codex/deepseek, no-bloqueante).
  const [rate, setRate] = useState(1.0);
  const [vol, setVol] = useState(1.0);
  // 033 U4 — opt-in de notificaciones en background (default OFF; al activar pide permiso del SO).
  const [notify, setNotify] = useState(false);
  // 034 U3 — sonido custom del toast de notificación ("" = sonido del SO).
  const [notifySound, setNotifySound] = useState("");
  // 034 U1 — traer Furx al frente al activar la app (clic en notif/dock); default OFF.
  const [bringToFront, setBringToFront] = useState(false);
  const load = async () => {
    try {
      const p = await invoke<AudioPrefs>("attention_audio_prefs_get", {});
      setPrefs(p); setRate(p.rate); setVol(p.earcon_volume);
    } catch { /* default UI oculta */ }
    try { setNotify(await invoke<boolean>("attention_notify_get_enabled", {})); } catch { /* default OFF */ }
    try { setNotifySound(await invoke<string>("attention_notify_sound_get", {})); } catch { /* default */ }
    try { setBringToFront(await invoke<boolean>("attention_notify_bring_to_front_get", {})); } catch { /* default OFF */ }
  };
  useEffect(() => { load(); }, []);
  if (!prefs) return null;
  const setNotifyEnabled = async (v: boolean) => {
    setNotify(v);
    try { await invoke("attention_notify_set_enabled", { enabled: v }); } catch { setNotify(!v); }
  };
  const save = async (patch: Record<string, unknown>) => {
    try { await invoke("attention_audio_prefs_set", patch); } catch { /* el backend ya valida; ignorar */ }
    await load();
  };
  return (
    <>
      <Toggle
        label="Notificar en background"
        hint="Notificación nativa cuando un agente necesita tu atención y Furx no tiene foco. No mueve el micrófono ni trae la ventana al frente. Al activar, macOS pedirá permiso."
        value={notify}
        onChange={setNotifyEnabled}
      />
      <Row
        label="Sonido de la notificación"
        value={notifySound}
        placeholder="ej. Ping — vacío = sonido del sistema"
        onSave={(v) => { setNotifySound(v); invoke("attention_notify_sound_set", { sound: v }).catch(() => {}); }}
      />
      <Toggle
        label="Traer Furx al frente al activar"
        hint="Al clickear una notificación (o el ícono del dock), trae la ventana de Furx al frente. No mueve el micrófono. Default desactivado."
        value={bringToFront}
        onChange={(v) => { setBringToFront(v); invoke("attention_notify_bring_to_front_set", { enabled: v }).catch(() => setBringToFront(!v)); }}
      />
      <Row
        label="Voz del TTS (macOS)"
        value={prefs.voice ?? ""}
        placeholder="ej. Mónica — vacío = voz del sistema"
        onSave={(v) => save({ voice: v })}
      />
      <div className="form-row">
        <label>Velocidad del TTS ({rate.toFixed(1)}×)</label>
        <div className="form-input">
          <input type="range" min={0.5} max={2} step={0.1} value={rate}
            onChange={(e) => setRate(parseFloat(e.target.value))}
            onPointerUp={() => save({ rate })}
            onKeyUp={() => save({ rate })} />
        </div>
      </div>
      <div className="form-row">
        <label>Volumen del earcon ({Math.round(vol * 100)}%)</label>
        <div className="form-input">
          <input type="range" min={0} max={1} step={0.05} value={vol}
            onChange={(e) => setVol(parseFloat(e.target.value))}
            onPointerUp={() => save({ earconVolume: vol })}
            onKeyUp={() => save({ earconVolume: vol })} />
        </div>
      </div>
    </>
  );
}

function TmuxWatchdogPanel({ busy, setBusy, setMsg }: { busy: boolean; setBusy: (b: boolean) => void; setMsg: (s: string | null) => void }) {
  const [status, setStatus] = useState<{ plist_path: string; installed: boolean; loaded: boolean; tmux_bin: string | null } | null>(null);
  const refresh = async () => {
    try { const s = await invoke<typeof status>("tmux_watchdog_status"); setStatus(s); } catch (e) { setMsg(`error: ${String(e)}`); }
  };
  useEffect(() => { refresh(); }, []);
  const install = async () => {
    setBusy(true); setMsg(null);
    try { await invoke("tmux_watchdog_install"); setMsg("watchdog instalado y cargado"); await refresh(); }
    catch (e) { setMsg(`error: ${String(e)}`); } finally { setBusy(false); }
  };
  const uninstall = async () => {
    setBusy(true); setMsg(null);
    try { await invoke("tmux_watchdog_uninstall"); setMsg("watchdog removido"); await refresh(); }
    catch (e) { setMsg(`error: ${String(e)}`); } finally { setBusy(false); }
  };
  if (!status) return <div className="muted">cargando…</div>;
  return (
    <>
      <div className="row-meta" style={{ fontFamily: "var(--mono)", fontSize: 11 }}>
        tmux: <code>{status.tmux_bin ?? "—"}</code> · plist: <code>{status.plist_path}</code> · installed: <strong>{String(status.installed)}</strong> · loaded: <strong>{String(status.loaded)}</strong>
      </div>
      <div className="actions-row" style={{ marginTop: 10 }}>
        <button onClick={install} disabled={busy || !status.tmux_bin}>{status.installed ? "Reinstall" : "Install"}</button>
        <Button variant="danger" onClick={uninstall} disabled={busy || !status.installed}>Uninstall</Button>
        <button onClick={refresh} disabled={busy}>Refresh</button>
      </div>
      {!status.tmux_bin && <div className="muted" style={{ marginTop: 6, fontSize: 11 }}>tmux no está en PATH — <code>brew install tmux</code> primero.</div>}
    </>
  );
}

function Item({ ok, label, req, install }: { ok: boolean; label: string; req: string; install?: string | null }) {
  const isUrl = install?.startsWith("http");
  return (
    <div className={`compat-item ${ok ? "ok" : "bad"}`}>
      <span className={`dot ${ok ? "up" : "down"}`} />
      <span>{label}</span>
      <span className="muted">{req}</span>
      {!ok && install && (
        isUrl ? (
          <a href={install} target="_blank" rel="noreferrer" className="install-link">Instalar</a>
        ) : (
          <button
            className="install-link"
            onClick={() => { navigator.clipboard.writeText(install); }}
            title="Copiar comando de instalación"
          >
            <code>{install.length > 30 ? install.slice(0, 28) + "…" : install}</code>
          </button>
        )
      )}
    </div>
  );
}

// ──────────────────────────────────────────────────────────────────────────
// 053 — paneles huérfanos cableados
// ──────────────────────────────────────────────────────────────────────────

// a) Pantallas — monitors_list() → ScreenInfo[]
interface ScreenInfo { id: string; x: number; y: number; width: number; height: number; scale_factor: number; is_primary: boolean }
function ScreensPanel() {
  const [screens, setScreens] = useState<ScreenInfo[] | null>(null);
  const [err, setErr] = useState<string | null>(null);
  const load = async () => {
    setErr(null);
    try { setScreens(await invoke<ScreenInfo[]>("monitors_list")); }
    catch (e) { setErr(String(e)); }
  };
  useEffect(() => { void load(); }, []);
  if (err) return <div className="muted">{err}</div>;
  if (!screens) return <div className="muted">cargando…</div>;
  if (screens.length === 0) return <div className="muted">Sin monitores detectados.</div>;
  return (
    <>
      <button onClick={load} style={{ marginBottom: 10 }}>Refrescar</button>
      <div className="compat-grid">
        {screens.map((s) => (
          <div key={s.id} className="compat-item ok">
            <span className={`dot ${s.is_primary ? "up" : "unknown"}`} />
            <span><strong>{s.is_primary ? "Principal" : "Secundario"}</strong> · {s.width}×{s.height}</span>
            <span className="muted">×{s.scale_factor.toFixed(1)} · ({s.x},{s.y})</span>
            <span className="muted" style={{ fontFamily: "var(--mono)", fontSize: 10 }}>{s.id}</span>
          </div>
        ))}
      </div>
    </>
  );
}

// b) Licencia — license_install_id() → string
function LicensePanel() {
  const [id, setId] = useState<string | null>(null);
  const [err, setErr] = useState<string | null>(null);
  const [copied, setCopied] = useState(false);
  useEffect(() => {
    invoke<string>("license_install_id")
      .then(setId)
      .catch((e) => setErr(String(e)));
  }, []);
  const copy = () => {
    if (!id) return;
    navigator.clipboard.writeText(id).catch(() => {});
    setCopied(true);
    setTimeout(() => setCopied(false), 1500);
  };
  if (err) return <div className="muted">{err}</div>;
  if (!id) return <div className="muted">cargando…</div>;
  return (
    <div className="row-meta">
      <span className="muted">Install ID:</span>{" "}
      <code style={{ fontFamily: "var(--mono)", fontSize: 11 }}>{id}</code>{" "}
      <button onClick={copy}>{copied ? "✓" : "Copiar"}</button>
    </div>
  );
}

// c) Spec-kit — spec_kit_alias_status() → boolean
function SpeckitPanel() {
  const [installed, setInstalled] = useState<boolean | null>(null);
  const [err, setErr] = useState<string | null>(null);
  useEffect(() => {
    invoke<boolean>("spec_kit_alias_status")
      .then(setInstalled)
      .catch((e) => setErr(String(e)));
  }, []);
  if (err) return <div className="muted">{err}</div>;
  if (installed === null) return <div className="muted">verificando…</div>;
  return (
    <div className="row-meta">
      <span className="muted">Alias `spec`:</span>{" "}
      <span
        style={{
          display: "inline-block",
          padding: "2px 8px",
          borderRadius: 4,
          fontSize: 12,
          fontWeight: 600,
          background: installed ? "var(--green, #2a7a2a)" : "var(--amber, #7a5500)",
          color: "#fff",
        }}
      >
        {installed ? "instalado" : "no instalado"}
      </span>
      {!installed && (
        <span className="muted" style={{ marginLeft: 8 }}>
          Instalá con: <code>furx install-spec-alias</code> o desde el wizard.
        </span>
      )}
    </div>
  );
}

// d) Sync manual — sync_run({remote?}) → SyncReport
interface SyncReport { commit: string | null; pushed: boolean; remote: string | null; bytes: number }
function SyncManualPanel() {
  const [remote, setRemote] = useState("");
  const [busy, setBusy] = useState(false);
  const [result, setResult] = useState<SyncReport | null>(null);
  const [err, setErr] = useState<string | null>(null);

  const run = async () => {
    setBusy(true); setErr(null); setResult(null);
    try {
      const r = await invoke<SyncReport>("sync_run", { remote: remote.trim() || null });
      setResult(r);
    } catch (e) {
      setErr(String(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <>
      <div className="form-row">
        <label>Remote (opcional)</label>
        <div className="form-input">
          <input
            value={remote}
            onChange={(e) => setRemote(e.target.value)}
            placeholder="git remote URL — vacío = default"
          />
        </div>
      </div>
      <button onClick={run} disabled={busy} style={{ marginTop: 8 }}>
        {busy ? "Sincronizando…" : "Sincronizar ahora"}
      </button>
      {err && <div className="muted" style={{ marginTop: 6, color: "var(--red, #c00)" }}>{err}</div>}
      {result && (
        <div className="row-meta" style={{ marginTop: 8, fontFamily: "var(--mono)", fontSize: 11 }}>
          commit: <code>{result.commit ?? "sin cambios"}</code>{" "}
          · pushed: <strong>{String(result.pushed)}</strong>{" "}
          · remote: <code>{result.remote ?? "—"}</code>{" "}
          · {(result.bytes / 1024).toFixed(1)} KB
        </div>
      )}
    </>
  );
}

// e) Preferencias — preference_records_list({limit: 20}) → PreferenceRecord[]
interface PreferenceRecord {
  id: string;
  group_id: string;
  repo_key: string;
  task_type: string;
  outcome_kind: string;
  feature_schema_version: number;
  revision: number | null;
  variants: Array<{ features: unknown; chosen: boolean }>;
}
function PreferencesPanel() {
  const [records, setRecords] = useState<PreferenceRecord[] | null>(null);
  const [err, setErr] = useState<string | null>(null);
  const load = async () => {
    setErr(null);
    try { setRecords(await invoke<PreferenceRecord[]>("preference_records_list", { limit: 20 })); }
    catch (e) { setErr(String(e)); }
  };
  useEffect(() => { void load(); }, []);

  if (err) return <div className="muted">{err}</div>;
  if (!records) return <div className="muted">cargando…</div>;
  if (records.length === 0) return <div className="muted">Sin registros de preferencia aún. Aparecen después de cerrar reviews best-of-N.</div>;

  return (
    <>
      <button onClick={load} style={{ marginBottom: 10 }}>Refrescar</button>
      <table style={{ width: "100%", borderCollapse: "collapse", fontSize: 11, fontFamily: "var(--mono)" }}>
        <thead>
          <tr style={{ borderBottom: "1px solid var(--border, #333)", textAlign: "left" }}>
            <th style={{ padding: "4px 8px" }}>ID</th>
            <th style={{ padding: "4px 8px" }}>task_type</th>
            <th style={{ padding: "4px 8px" }}>outcome</th>
            <th style={{ padding: "4px 8px" }}>repo</th>
            <th style={{ padding: "4px 8px" }}>variantes</th>
          </tr>
        </thead>
        <tbody>
          {records.map((r) => (
            <tr key={r.id} style={{ borderBottom: "1px solid var(--border, #222)" }}>
              <td style={{ padding: "4px 8px" }}><code style={{ fontSize: 10 }}>{r.id.slice(0, 8)}</code></td>
              <td style={{ padding: "4px 8px" }}>{r.task_type}</td>
              <td style={{ padding: "4px 8px" }}>{r.outcome_kind}</td>
              <td style={{ padding: "4px 8px" }} className="muted">{r.repo_key.slice(0, 12)}</td>
              <td style={{ padding: "4px 8px" }}>{r.variants.length} ({r.variants.filter((v) => v.chosen).length} elegida)</td>
            </tr>
          ))}
        </tbody>
      </table>
    </>
  );
}

// f) Audio · Atención — opt-in por pane
function AudioOptInPanel() {
  const [paneId, setPaneId] = useState("");
  const [enabled, setEnabled] = useState<boolean | null>(null);
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState<string | null>(null);

  const load = async (id: string) => {
    if (!id.trim()) return;
    setErr(null);
    try {
      const v = await invoke<boolean>("attention_audio_opt_in_get", { paneId: id.trim() });
      setEnabled(v);
    } catch (e) {
      setErr(String(e));
    }
  };

  const toggle = async (v: boolean) => {
    if (!paneId.trim()) return;
    setBusy(true); setErr(null);
    try {
      await invoke("attention_audio_opt_in_set", { paneId: paneId.trim(), enabled: v });
      setEnabled(v);
    } catch (e) {
      setErr(String(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <>
      <div className="form-row">
        <label>Pane ID</label>
        <div className="form-input">
          <input
            value={paneId}
            onChange={(e) => { setPaneId(e.target.value); setEnabled(null); }}
            placeholder="ej. pane-1 — el ID del pane en Furx"
          />
          <button onClick={() => load(paneId)} disabled={!paneId.trim()}>Leer</button>
        </div>
      </div>
      {err && <div className="muted" style={{ color: "var(--red, #c00)", marginTop: 4 }}>{err}</div>}
      {enabled !== null && (
        <div className="form-row toggle-row" style={{ marginTop: 8 }}>
          <label>
            <input
              type="checkbox"
              checked={enabled}
              disabled={busy}
              onChange={(e) => toggle(e.target.checked)}
            />{" "}
            Audio de atención para <code>{paneId}</code>
          </label>
          <div className="hint muted">Activa el aviso de audio cuando este pane requiere atención.</div>
        </div>
      )}
    </>
  );
}

// g) Proveedores · búsqueda — provider_get({alias}) → ProviderCredential | null
interface ProviderCredential {
  alias: string;
  provider: string;
  key_ref: string | null;
  endpoint_url: string | null;
  status: string;
  last_ping_ms: number | null;
  last_ping_at: string | null;
  last_error_msg: string | null;
  scope_workspace: string | null;
  preset_member: string | null;
  created_at: string;
  updated_at: string;
}
// 059 — rebind del hotkey de push-to-talk. Muestra el combo actual y permite grabar uno nuevo:
// "Cambiar" → captura el próximo keydown completo (modificador(es) + tecla base) y lo persiste.
// Mientras graba, marca `setPttCapturing(true)` para que el handler global de PTT no dispare voz.
function PttHotkeyRow({ current, onSave }: { current: string; onSave: (combo: string) => void | Promise<void> }) {
  const [recording, setRecording] = useState(false);
  const label = formatPttHotkey(parsePttHotkey(current));

  useEffect(() => {
    if (!recording) return;
    setPttCapturing(true);
    const onKey = (e: KeyboardEvent) => {
      e.preventDefault();
      e.stopPropagation();
      if (e.key === "Escape") { setRecording(false); return; }
      const combo = eventToHotkeyString(e);
      if (!combo) return; // sólo-modificador → seguir esperando la tecla base
      setRecording(false);
      void onSave(combo);
    };
    // capture phase para ganarle a cualquier otro handler.
    window.addEventListener("keydown", onKey, true);
    return () => {
      window.removeEventListener("keydown", onKey, true);
      setPttCapturing(false);
    };
  }, [recording, onSave]);

  return (
    <div className="row">
      <div>
        <div className="row-label">Push-to-talk</div>
        <div className="row-meta"><span className="muted">Mantené el atajo para grabar voz; soltá para transcribir.</span></div>
      </div>
      <div style={{ display: "flex", gap: 8, alignItems: "center" }}>
        <code style={{ fontFamily: "var(--font-mono)", fontSize: 13 }}>{recording ? "presioná tu combinación… (Esc cancela)" : label}</code>
        <Button onClick={() => setRecording((r) => !r)}>{recording ? "Cancelar" : "Cambiar"}</Button>
      </div>
    </div>
  );
}

function ProviderLookupPanel() {
  const [alias, setAlias] = useState("");
  const [cred, setCred] = useState<ProviderCredential | null | undefined>(undefined);
  const [err, setErr] = useState<string | null>(null);

  const lookup = async () => {
    if (!alias.trim()) return;
    setErr(null); setCred(undefined);
    try {
      const r = await invoke<ProviderCredential | null>("provider_get", { alias: alias.trim() });
      setCred(r);
    } catch (e) {
      setErr(String(e));
    }
  };

  const statusColor = (s: string) =>
    s === "healthy" ? "var(--green, #2a7a2a)" : s === "amber" ? "var(--amber, #7a5500)" : s === "red" ? "var(--red, #c00)" : "var(--text-muted)";

  return (
    <>
      <div className="form-row">
        <label>Alias</label>
        <div className="form-input">
          <input
            value={alias}
            onChange={(e) => setAlias(e.target.value)}
            placeholder="ej. anthropic-claude"
            onKeyDown={(e) => { if (e.key === "Enter") void lookup(); }}
          />
          <button onClick={lookup} disabled={!alias.trim()}>Buscar</button>
        </div>
      </div>
      {err && <div className="muted" style={{ color: "var(--red, #c00)", marginTop: 4 }}>{err}</div>}
      {cred === null && <div className="muted" style={{ marginTop: 8 }}>Proveedor <code>{alias}</code> no encontrado.</div>}
      {cred && (
        <div style={{ marginTop: 10, fontFamily: "var(--mono)", fontSize: 11 }}>
          <div className="row-meta">
            <strong>{cred.alias}</strong>{" "}
            <span style={{ color: statusColor(cred.status), fontWeight: 600 }}>{cred.status}</span>{" "}
            <span className="muted">({cred.provider})</span>
          </div>
          {cred.endpoint_url && <div className="row-meta muted">endpoint: <code>{cred.endpoint_url}</code></div>}
          {cred.last_ping_ms !== null && (
            <div className="row-meta muted">
              last ping: <code>{cred.last_ping_ms}ms</code>
              {cred.last_ping_at && <> · <code>{cred.last_ping_at}</code></>}
            </div>
          )}
          {cred.last_error_msg && <div className="row-meta" style={{ color: "var(--red, #c00)" }}>error: {cred.last_error_msg}</div>}
          {cred.scope_workspace && <div className="row-meta muted">workspace: <code>{cred.scope_workspace}</code></div>}
          {cred.preset_member && <div className="row-meta muted">preset: <code>{cred.preset_member}</code></div>}
          <div className="row-meta muted">creado: {cred.created_at} · actualizado: {cred.updated_at}</div>
        </div>
      )}
    </>
  );
}


