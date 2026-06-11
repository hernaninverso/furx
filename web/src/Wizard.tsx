// Furx onboarding · BYOK-universal.
// REWRITE 2026-05-26: el wizard previo mencionaba "claude-max-A/B" + scripts personales
// del dev — setup específico del dev, NO del usuario final. Ahora es agnóstico.
// 042 FR-002: paso "endpoints" entre privacy y connect (el usuario configura su AIE/Ollama sin
// tocar código). Foco humano — el wizard GUÍA y persiste lo que el usuario confirma, no auto-configura.
// 2026-06-09 brand wave 4: todo el copy user-facing pasa por i18n (keys wizard.* en locales/),
// EN default + ES elegible. Sin texto hardcodeado en este archivo.

import { useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Button } from "./components/Button";
import { markFirstRunCompletedLocal } from "./lib/boot"; // 042 FR-005 — fallsafe anti-loop.
import { useT } from "./lib/i18n";

// 042 FR-004 — resultado del wizard. `openConnect`: abrir Furx Connect tras cerrar. `firstPaneMode`:
// si el usuario eligió crear su primer pane en el paso 4, el modo ("zsh" o un agente CLI); null si no.
export interface WizardResult {
  openConnect: boolean;
  firstPaneMode: string | null;
}

interface Props {
  onDone: (result: WizardResult) => void;
  // 042 FR-005 — cerrar el wizard con la X SIN completarlo (re-aparece al próximo arranque, salvo
  // que un finish() haya fallado esta sesión → ahí el fallsafe local evita el bucle).
  onClose: () => void;
}

type Step = "welcome" | "privacy" | "endpoints" | "connect" | "firstpane";

// 042 FR-002 — resultado del health-check de un endpoint (espejo de wizard::HealthResult de Rust).
interface HealthResult { reachable: boolean; latency_ms: number | null; error: string | null }
interface HealthPair { aie: HealthResult; ollama: HealthResult }

const AIE_PLACEHOLDER = "http://localhost:8250";
const OLLAMA_PLACEHOLDER = "http://localhost:11434";

export function Wizard({ onDone, onClose }: Props) {
  const t = useT();
  const [step, setStep] = useState<Step>("welcome");
  const [telemetry, setTelemetry] = useState(false);
  const [eulaAccepted, setEulaAccepted] = useState(false);
  const [busy, setBusy] = useState(false);
  // 042 FR-005 — error de finish() SURFACEADO (no console.error tragado) + recuento de fallos para
  // el fallsafe: tras >=2 fallos ofrecemos "Finalizar de todas formas" (marca el flag local + cierra).
  const [finishError, setFinishError] = useState<string | null>(null);
  const [finishFailCount, setFinishFailCount] = useState(0);
  const [localWarning, setLocalWarning] = useState(false);

  // 042 FR-002 — campos de endpoints (VACÍOS con placeholder; "" = usar el default localhost).
  const [aieUrl, setAieUrl] = useState("");
  const [ollamaUrl, setOllamaUrl] = useState("");
  const [checking, setChecking] = useState(false);
  const [health, setHealth] = useState<HealthPair | null>(null);
  const [endpointError, setEndpointError] = useState<string | null>(null);

  // 042 FR-004 — si el usuario quiso abrir Connect (botón del paso connect) se recuerda acá y se
  // aplica al cerrar desde el paso firstpane.
  const [openConnect, setOpenConnect] = useState(false);

  const finish = async (firstPaneMode: string | null) => {
    setBusy(true);
    setFinishError(null);
    try {
      await invoke("settings_set", { key: "opt_in.telemetry", value: telemetry });
      await invoke("settings_set", { key: "opt_in.eula_accepted_at", value: new Date().toISOString() });
      await invoke("settings_set", { key: "app.first_run_completed", value: true });
      // 042 FR-005 (audit codex HIGH cross-fase) — ESPEJO local del flag de DB también en el camino
      // de ÉXITO: si un boot POSTERIOR no puede leer settings (settings_get falla transitoriamente),
      // el boot cae a `error` con `needsWizard=!firstRunCompletedLocal()` → sin este mirror, un wizard
      // YA completado se re-abriría. Best-effort (no bloquea el éxito si localStorage no persiste).
      markFirstRunCompletedLocal();
      onDone({ openConnect, firstPaneMode });
    } catch (e) {
      // 042 FR-005 — error SURFACEADO (no tragado). El modal NO se cierra: el usuario ve qué falló y
      // puede reintentar. first_run_completed NO quedó seteado en DB → sin el fallsafe, el wizard
      // re-aparecería; el botón "Finalizar de todas formas" (tras 2 fallos) escribe el flag local.
      console.error("wizard finish failed:", e); // ademas del UI, para debugging (no en vez de).
      setFinishError(String(e));
      setFinishFailCount((n) => n + 1);
    } finally {
      setBusy(false);
    }
  };

  // 042 FR-005 — fallsafe anti-loop: el usuario decide salir pese al fallo de DB. Marcamos el flag
  // local (que el boot lee además del de DB) para no re-abrir el wizard en bucle, y cerramos. Si ni
  // siquiera localStorage persiste (quota / modo privado), avisamos cómo completar.
  const finishAnyway = (firstPaneMode: string | null) => {
    const ok = markFirstRunCompletedLocal();
    if (!ok) { setLocalWarning(true); return; }
    onDone({ openConnect, firstPaneMode });
  };

  // 042 FR-005 — cerrar con la X. Si un finish() ya falló esta sesión, el flag de DB no se escribió,
  // así que ANTES de cerrar marcamos el fallsafe local para que el wizard NO re-aparezca en bucle. Si
  // NI localStorage persiste (quota / modo privado), NO cerramos a ciegas: surfaceamos el warning y
  // dejamos el wizard abierto (cerrar igual lo re-abriría en cada arranque — el peor caso del bucle).
  const closeWithX = () => {
    if (finishFailCount > 0) {
      if (!markFirstRunCompletedLocal()) { setLocalWarning(true); return; }
    }
    onClose();
  };

  // 042 FR-002 — "Probar": health-check async (backend: timeout 1500ms, sin redirect). Si ambos
  // campos están vacíos, probamos los defaults localhost (lo que la app usará si se saltea).
  const probe = async () => {
    setChecking(true);
    setEndpointError(null);
    setHealth(null);
    try {
      const pair = await invoke<HealthPair>("setup_health_check", {
        aieUrl: aieUrl.trim() || AIE_PLACEHOLDER,
        ollamaUrl: ollamaUrl.trim() || OLLAMA_PLACEHOLDER,
      });
      setHealth(pair);
    } catch (e) {
      // Error SURFACEADO (no tragado): el usuario ve por qué falló la prueba.
      setEndpointError(String(e));
    } finally {
      setChecking(false);
    }
  };

  // 042 FR-002 — "Guardar y continuar": si hay al menos un campo no vacío, persiste vía
  // wizard_save_endpoints (valida con url::Url + agrega a la allowlist). Vacío = equivale a saltear.
  const saveAndContinue = async () => {
    const aie = aieUrl.trim();
    const ollama = ollamaUrl.trim();
    if (!aie && !ollama) { setStep("connect"); return; }
    setBusy(true);
    setEndpointError(null);
    try {
      await invoke("wizard_save_endpoints", { aieUrl: aie, ollamaUrl: ollama });
      setStep("connect");
    } catch (e) {
      setEndpointError(String(e));
    } finally {
      setBusy(false);
    }
  };

  const stepsOrder: Step[] = ["welcome", "privacy", "endpoints", "connect", "firstpane"];
  const stepIdx = stepsOrder.indexOf(step);

  const dotClass = (r: HealthResult | undefined) =>
    `health-dot ${r ? (r.reachable ? "ok" : "bad") : ""}`;
  const healthLine = (label: string, r: HealthResult | undefined) => (
    <div className="health-line">
      <span className={dotClass(r)} aria-hidden="true" />
      {label}:{" "}
      {!r
        ? "—"
        : r.reachable
          ? t("wizard.health.ok", { ms: r.latency_ms ?? "?" })
          : t("wizard.health.bad", { error: r.error ?? "error" })}
    </div>
  );

  return (
    <div className="wizard-backdrop">
      <div className="wizard">
        <header className="wizard-header">
          <span className="hex" />
          <div>
            <h2>{t("wizard.title")}</h2>
            <div className="muted">{t("wizard.stepOf", { n: stepIdx + 1, total: stepsOrder.length })}</div>
          </div>
          <div className="wizard-steps">
            {stepsOrder.map((s, i) => (
              <span
                key={s}
                className={`step-dot ${step === s ? "active" : ""} ${stepIdx > i ? "done" : ""}`}
                onClick={() => stepIdx > i && setStep(s)}
                role="button"
                aria-label={t("wizard.stepGo", { n: i + 1 })}
              />
            ))}
          </div>
          {/* 042 FR-005 — cerrar el wizard sin completarlo (X). */}
          <button
            type="button"
            className="modal-close-x"
            aria-label={t("wizard.close")}
            onClick={closeWithX}
            disabled={busy}
          >
            ×
          </button>
        </header>

        <main className="wizard-body">
          {step === "welcome" && (
            <>
              <h3>{t("wizard.welcome.title")}</h3>
              <p>
                {t("wizard.welcome.p1pre")} <strong>Council Mode</strong> {t("wizard.welcome.p1post")}
              </p>
              <p className="muted small">{t("wizard.welcome.p2")}</p>
              <ul className="wizard-bullets">
                <li>{t("wizard.welcome.b1")}</li>
                <li>{t("wizard.welcome.b2")}</li>
                <li>{t("wizard.welcome.b3")}</li>
              </ul>
              <div className="wizard-actions">
                <span style={{ flex: 1 }} />
                <Button variant="primary" onClick={() => setStep("privacy")}>{t("wizard.continue")}</Button>
              </div>
            </>
          )}

          {step === "privacy" && (
            <>
              <h3>{t("wizard.privacy.title")}</h3>
              <p>{t("wizard.privacy.p1")}</p>
              <label className="check">
                <input
                  type="checkbox"
                  checked={telemetry}
                  onChange={(e) => setTelemetry(e.target.checked)}
                />{" "}
                {t("wizard.privacy.telemetry")}
              </label>

              <details style={{ marginTop: 14 }}>
                <summary>{t("wizard.privacy.licenseSummary")}</summary>
                <p className="muted small" style={{ marginTop: 8 }}>
                  {t("wizard.privacy.licenseText")}
                </p>
              </details>

              <label className="check" style={{ marginTop: 8 }}>
                <input
                  type="checkbox"
                  checked={eulaAccepted}
                  onChange={(e) => setEulaAccepted(e.target.checked)}
                />{" "}
                {t("wizard.privacy.accept")}
              </label>

              <div className="wizard-actions">
                <button onClick={() => setStep("welcome")}>{t("wizard.back")}</button>
                <Button variant="primary" disabled={!eulaAccepted} onClick={() => setStep("endpoints")}>
                  {t("wizard.continue")}
                </Button>
              </div>
            </>
          )}

          {step === "endpoints" && (
            <>
              <h3>{t("wizard.endpoints.title")}</h3>
              <p>{t("wizard.endpoints.p1")}</p>

              <div className="wizard-field">
                <label htmlFor="wiz-aie">AI Engine</label>
                <input
                  id="wiz-aie"
                  type="text"
                  value={aieUrl}
                  placeholder={AIE_PLACEHOLDER}
                  onChange={(e) => { setAieUrl(e.target.value); setHealth(null); }}
                  spellCheck={false}
                  autoCapitalize="off"
                  autoCorrect="off"
                />
              </div>
              <div className="wizard-field">
                <label htmlFor="wiz-ollama">{t("wizard.endpoints.ollamaLabel")}</label>
                <input
                  id="wiz-ollama"
                  type="text"
                  value={ollamaUrl}
                  placeholder={OLLAMA_PLACEHOLDER}
                  onChange={(e) => { setOllamaUrl(e.target.value); setHealth(null); }}
                  spellCheck={false}
                  autoCapitalize="off"
                  autoCorrect="off"
                />
              </div>

              {health && (
                <div style={{ marginTop: 10 }} role="status" aria-live="polite">
                  {healthLine("AI Engine", health.aie)}
                  {healthLine("Ollama", health.ollama)}
                </div>
              )}
              {endpointError && (
                <div role="alert" className="muted small" style={{ marginTop: 8, color: "var(--red, #e05555)" }}>
                  {endpointError}
                </div>
              )}

              <div className="wizard-actions">
                <button onClick={() => setStep("privacy")}>{t("wizard.back")}</button>
                <button onClick={probe} disabled={checking}>
                  {checking ? t("wizard.endpoints.testing") : t("wizard.endpoints.test")}
                </button>
                <button onClick={() => setStep("connect")} disabled={busy}>{t("wizard.endpoints.skip")}</button>
                <Button variant="primary" onClick={saveAndContinue} disabled={busy}>
                  {t("wizard.endpoints.saveContinue")}
                </Button>
              </div>
            </>
          )}

          {step === "connect" && (
            <>
              <h3>{t("wizard.connect.title")}</h3>
              <p>{t("wizard.connect.p1")}</p>
              <ul className="wizard-bullets">
                <li>{t("wizard.connect.b1")}</li>
                <li>{t("wizard.connect.b2")}</li>
                <li>{t("wizard.connect.b3")}</li>
                <li>{t("wizard.connect.b4")}</li>
                <li>{t("wizard.connect.b5")}</li>
              </ul>
              <p className="muted small">{t("wizard.connect.skipHint")}</p>
              <div className="wizard-actions">
                <button onClick={() => setStep("endpoints")}>{t("wizard.back")}</button>
                <button onClick={() => { setOpenConnect(false); setStep("firstpane"); }} disabled={busy}>
                  {t("wizard.connect.skipNow")}
                </button>
                <Button variant="primary" onClick={() => { setOpenConnect(true); setStep("firstpane"); }} disabled={busy}>
                  {t("wizard.connect.open")}
                </Button>
              </div>
            </>
          )}

          {step === "firstpane" && (
            <>
              <h3>{t("wizard.firstpane.title")}</h3>
              <p>{t("wizard.firstpane.p1")}</p>
              <ul className="wizard-bullets">
                <li>{t("wizard.firstpane.b1")}</li>
                <li>{t("wizard.firstpane.b2")}</li>
              </ul>
              <p className="muted small">{t("wizard.firstpane.explore")}</p>

              {/* 042 FR-005 — error de finish() SURFACEADO + fallsafe tras fallos repetidos. */}
              {finishError && (
                <div role="alert" className="small" style={{ marginTop: 8, color: "var(--red, #e05555)" }}>
                  {t("wizard.finish.error", { error: finishError })}
                  {finishFailCount >= 2 && t("wizard.finish.errorAnyway")}
                </div>
              )}
              {localWarning && (
                <div role="alert" className="small" style={{ marginTop: 8, color: "var(--amber, #d8a000)" }}>
                  {t("wizard.finish.localWarning")}
                </div>
              )}

              <div className="wizard-actions">
                <button onClick={() => setStep("connect")} disabled={busy}>{t("wizard.back")}</button>
                <button onClick={() => finish(null)} disabled={busy}>{t("wizard.firstpane.finish")}</button>
                <Button variant="primary" onClick={() => finish("zsh")} disabled={busy}>
                  {t("wizard.firstpane.openZsh")}
                </Button>
              </div>
              {finishFailCount >= 2 && (
                <div className="wizard-actions" style={{ marginTop: 6 }}>
                  <span style={{ flex: 1 }} />
                  <button onClick={() => finishAnyway(null)} disabled={busy}>{t("wizard.finish.anyway")}</button>
                </div>
              )}
            </>
          )}
        </main>
      </div>
    </div>
  );
}
