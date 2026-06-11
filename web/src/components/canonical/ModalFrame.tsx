/* ModalFrame — componente canónico de modal (US8, spec 015).
 *
 * Frame estándar: header (título + subtítulo + close ×), body scrollable,
 * footer de acciones. Reusa el <Modal> probado para la mecánica difícil
 * (focus-trap, ESC, backdrop, body-scroll-lock, portal, z-index stack) en vez
 * de reimplementarla — así migrar los ~35 modales no arriesga regresiones de
 * accesibilidad. ModalFrame solo aporta la ESTRUCTURA + estética V3 atelier.
 *
 * Estados soportados (vía props): loading, error, danger (tono destructivo).
 * Sólo usa tokens semánticos (clases fxc-modal__* de canonical.css).
 */

import { ReactNode, RefObject } from "react";
import { Modal } from "../Modal";

export interface ModalFrameProps {
  /** Título mostrado en el header (Fraunces). */
  title: string;
  /** Subtítulo opcional (Space Mono, muted). */
  subtitle?: ReactNode;
  /** Cerrar (ESC, backdrop, botón ×). */
  onClose: () => void;
  /** Contenido del body (scrollable). */
  children: ReactNode;
  /** Acciones del footer (botones). Si se omite, no se renderiza footer. */
  footer?: ReactNode;
  /** Submit con ⌘/Ctrl+Enter (delegado a Modal). */
  onSubmit?: () => void;
  canSubmit?: boolean;
  /** Ancho máximo del diálogo. */
  maxWidth?: number | string;
  /** Tono destructivo: hairlines clay + título clay. */
  danger?: boolean;
  /** Estado de carga: muestra spinner en el body en vez de children. */
  loading?: boolean;
  /** Mensaje de error: lo muestra como bloque error sobre los children. */
  error?: string | null;
  /** aria-label override (default = title). */
  ariaLabel?: string;
  /** Cerrar al click en backdrop (default true). */
  closeOnBackdrop?: boolean;
  /** Foco inicial dentro del modal. */
  initialFocusRef?: RefObject<HTMLElement | null>;
}

export function ModalFrame({
  title,
  subtitle,
  onClose,
  children,
  footer,
  onSubmit,
  canSubmit,
  maxWidth = 640,
  danger = false,
  loading = false,
  error = null,
  ariaLabel,
  closeOnBackdrop = true,
  initialFocusRef,
}: ModalFrameProps) {
  return (
    <Modal
      title={title}
      ariaLabel={ariaLabel ?? title}
      onClose={onClose}
      onSubmit={onSubmit}
      canSubmit={canSubmit}
      maxWidth={maxWidth}
      closeOnBackdrop={closeOnBackdrop}
      initialFocusRef={initialFocusRef}
      showHeader={false}
    >
      <div className={`fxc-modal${danger ? " fxc-modal--danger" : ""}`}>
        <header className="fxc-modal__header">
          <div className="fxc-modal__heading">
            <h2 className="fxc-modal__title">{title}</h2>
            {subtitle != null && <div className="fxc-modal__subtitle">{subtitle}</div>}
          </div>
          <button
            type="button"
            className="modal-close-x"
            onClick={onClose}
            aria-label="Close"
            title="Close (Esc)"
            data-modal-close-button="true"
            style={{ position: "static" }}
          >
            ×
          </button>
        </header>

        <div className="fxc-modal__body">
          {error && (
            <div className="fxc-state fxc-state--error" role="alert">
              {error}
            </div>
          )}
          {loading ? (
            <div className="fxc-state" role="status">
              <span className="fxc-spinner" aria-hidden="true" />
              <span>cargando…</span>
            </div>
          ) : (
            children
          )}
        </div>

        {footer != null && <footer className="fxc-modal__footer">{footer}</footer>}
      </div>
    </Modal>
  );
}
