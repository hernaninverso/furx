/* EmptyState — estado vacío accionable canónico (US8, spec 015).
 *
 * Icono · título · descripción · acción opcional. Variante `error` para
 * "no se pudo cargar / falló". Reemplaza los empty-states ad-hoc dispersos.
 * Sólo tokens.
 */

import { ReactNode } from "react";

export interface EmptyStateProps {
  /** Título (Fraunces). */
  title: ReactNode;
  /** Descripción / siguiente paso sugerido. */
  description?: ReactNode;
  /** Icono o glifo (string corto o node). Default: glifo editorial "✱". */
  icon?: ReactNode;
  /** Acción primaria (típicamente un botón). */
  action?: ReactNode;
  /** Variante error: tinte danger en icono/título. */
  variant?: "default" | "error";
}

export function EmptyState({
  title,
  description,
  icon = "✱",
  action,
  variant = "default",
}: EmptyStateProps) {
  return (
    <div className={`fxc-empty${variant === "error" ? " fxc-empty--error" : ""}`} role="status">
      <span className="fxc-empty__icon" aria-hidden="true">
        {icon}
      </span>
      <div className="fxc-empty__title">{title}</div>
      {description != null && <div className="fxc-empty__desc">{description}</div>}
      {action != null && <div className="fxc-empty__action">{action}</div>}
    </div>
  );
}
