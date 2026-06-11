// BLOQUE 1 · License gate UI shown when a Pro-only feature is triggered
// without an active license/trial.

import type { LicenseState } from "../types";
import { Modal } from "./Modal";
import { Button } from "./Button";

interface Props {
  feature: string;
  licenseState: LicenseState | null;
  onConnect: () => void;
  onClose: () => void;
}

export function ProGateModal({ feature, licenseState, onConnect, onClose }: Props) {
  const subtitle = describeState(licenseState);
  return (
    <Modal title={`${feature} es una feature Pro`} subtitle={subtitle} maxWidth={540} onClose={onClose} onSubmit={onConnect}>
      <p>
        Para usar <strong>{feature}</strong> necesitás un Furx Pro activo o estar en trial. Pro
        corre 4-6 LLMs en paralelo via tu propio Council Mode (BYOK universal — OpenRouter,
        free tiers individuales, APIs pagas, o modelos locales como Ollama).
      </p>
      <p className="muted small">
        Tu trial de 14 días empezó al instalar Furx. Si todavía está activo, podés usar Council
        ya. Si expiró, suscribite a Pro $12/mes para reactivarlo.
      </p>
      <div className="wizard-actions">
        <button onClick={onClose}>Cerrar</button>
        <Button variant="primary" onClick={onConnect}>
          Conectar provider y empezar
        </Button>
      </div>
    </Modal>
  );
}

function describeState(s: LicenseState | null): string {
  if (!s) return "Estado de licencia desconocido (offline).";
  switch (s.kind) {
    case "valid":
      return `Pro ${s.tier} válido hasta ${shortDate(s.until)}`;
    case "trial":
      return `Trial activo hasta ${shortDate(s.until)}`;
    case "expired":
      return "Tu licencia o trial expiró.";
    case "offline":
      return `Modo offline (último check: ${s.last_check === "never" ? "nunca" : shortDate(s.last_check)})`;
  }
}

function shortDate(iso: string): string {
  try {
    const d = new Date(iso);
    return d.toLocaleDateString();
  } catch {
    return iso;
  }
}
