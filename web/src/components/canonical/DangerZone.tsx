/* DangerZone — sección de acciones destructivas con confirmación visual
 * (US8, spec 015). Alinea con F-VI estética V3 + VI "parar ante destructivo".
 *
 * Encabezado clay (label DANGER + título) · descripción · acciones. Soporta
 * una confirmación tipeada opcional (`confirmPhrase`): el usuario debe escribir
 * la frase exacta para habilitar la acción — el componente expone `confirmed`
 * vía render-prop para que el consumidor deshabilite su botón.
 *
 * NO ejecuta nada: solo aporta la envoltura visual + el gate de confirmación
 * de UI. La puerta REAL de permisos vive en Rust (US4). Sólo tokens.
 */

import { ReactNode, useId, useState } from "react";

export interface DangerZoneProps {
  /** Título de la zona (p.ej. "Delete account"). */
  title: ReactNode;
  /** Descripción de la consecuencia. */
  description?: ReactNode;
  /** Label de la zona (mono). Default "DANGER ZONE". */
  label?: string;
  /**
   * Acciones. Si se pasa `confirmPhrase`, usar la forma render-prop para
   * recibir `confirmed` y deshabilitar el botón hasta que coincida la frase.
   */
  children?: ReactNode | ((state: { confirmed: boolean }) => ReactNode);
  /**
   * Frase que el usuario debe tipear exacto para confirmar. Si se omite, no
   * hay gate tipeado (la confirmación queda a cargo del consumidor / del modal).
   */
  confirmPhrase?: string;
}

export function DangerZone({
  title,
  description,
  label = "DANGER ZONE",
  children,
  confirmPhrase,
}: DangerZoneProps) {
  const [typed, setTyped] = useState("");
  const inputId = useId();
  const confirmed = confirmPhrase == null ? true : typed.trim() === confirmPhrase;

  const actions =
    typeof children === "function" ? children({ confirmed }) : children;

  return (
    <section className="fxc-danger-zone" aria-label={label}>
      <div className="fxc-danger-zone__header">
        <span className="fxc-danger-zone__label">{label}</span>
        <span className="fxc-danger-zone__title">{title}</span>
      </div>
      {description != null && <p className="fxc-danger-zone__desc">{description}</p>}

      {confirmPhrase != null && (
        <div className="fxc-danger-zone__confirm">
          <label className="fxc-danger-zone__confirm-label" htmlFor={inputId}>
            Escribí <code>{confirmPhrase}</code> para confirmar.
          </label>
          <input
            id={inputId}
            className="fxc-danger-zone__input"
            type="text"
            value={typed}
            onChange={(e) => setTyped(e.target.value)}
            autoComplete="off"
            spellCheck={false}
            aria-label={`Confirmación: escribir "${confirmPhrase}"`}
          />
        </div>
      )}

      {actions != null && <div className="fxc-danger-zone__actions">{actions}</div>}
    </section>
  );
}
