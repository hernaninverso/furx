/* PageHeader — encabezado canónico de vista/página (US8, spec 015).
 *
 * Eyebrow (label mono uppercase) · título (Fraunces, soporta <em> italic) ·
 * descripción opcional · slot de acciones a la derecha. Sólo tokens.
 */

import { ReactNode } from "react";

export interface PageHeaderProps {
  /** Título principal (Fraunces). Acepta nodes para <em> editorial. */
  title: ReactNode;
  /** Label mono uppercase sobre el título (dominio/sección). */
  eyebrow?: ReactNode;
  /** Descripción de una línea o dos. */
  description?: ReactNode;
  /** Acciones (botones) alineadas a la derecha. */
  actions?: ReactNode;
}

export function PageHeader({ title, eyebrow, description, actions }: PageHeaderProps) {
  return (
    <header className="fxc-page-header">
      <div className="fxc-page-header__lead">
        {eyebrow != null && <div className="fxc-page-header__eyebrow">{eyebrow}</div>}
        <h1 className="fxc-page-header__title">{title}</h1>
        {description != null && <p className="fxc-page-header__desc">{description}</p>}
      </div>
      {actions != null && <div className="fxc-page-header__actions">{actions}</div>}
    </header>
  );
}
