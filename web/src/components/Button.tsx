// 022 US9 (FR-014) — <Button> design-system con escala CERRADA de variantes.
// ──────────────────────────────────────────────────────────────────────────
// Reemplaza los `<button className="ghost|primary|danger|mini…">` ad-hoc por un
// único componente con variantes semánticas tipadas. Esto mata las "formas
// caprichosas": un único radio/altura por size, color SOLO desde tokens V3
// (styles/buttonComponent.css). Accesible: rol nativo <button>, foco visible,
// aria-busy en loading, aria-disabled coherente.
//
// Reglas:
//  - NO cambia comportamiento: onClick/disabled/type pasan tal cual.
//  - `className` queda como ESCAPE documentado para layout (márgenes), nunca
//    para color/forma — se anexa después de las clases canónicas.
import { ButtonHTMLAttributes, ReactNode, forwardRef } from "react";
import { buttonClasses, ButtonVariant, ButtonSize } from "../lib/buttonVariants.ts";

export type { ButtonVariant, ButtonSize };

export interface ButtonProps extends Omit<ButtonHTMLAttributes<HTMLButtonElement>, "type"> {
  /** Variante semántica (escala cerrada). Default: secondary. */
  variant?: ButtonVariant;
  /** Tamaño (escala cerrada). Default: md. */
  size?: ButtonSize;
  /** Muestra spinner + aria-busy; deshabilita interacción. */
  loading?: boolean;
  /** Ícono opcional a la izquierda del label. */
  iconLeft?: ReactNode;
  /** Ícono opcional a la derecha del label. */
  iconRight?: ReactNode;
  /** type nativo del botón. Default "button" (evita submits accidentales). */
  type?: "button" | "submit" | "reset";
}

export const Button = forwardRef<HTMLButtonElement, ButtonProps>(function Button(
  {
    variant = "secondary",
    size = "md",
    loading = false,
    iconLeft,
    iconRight,
    type = "button",
    disabled,
    className,
    children,
    "aria-label": ariaLabel,
    ...rest
  },
  ref
) {
  const isDisabled = disabled || loading;
  return (
    <button
      ref={ref}
      type={type}
      className={buttonClasses({ variant, size, loading, className })}
      disabled={isDisabled}
      aria-disabled={isDisabled || undefined}
      aria-busy={loading || undefined}
      aria-label={ariaLabel}
      {...rest}
    >
      {loading && <span className="fx-button__spinner" aria-hidden="true" />}
      {!loading && iconLeft && <span className="fx-button__icon" aria-hidden="true">{iconLeft}</span>}
      {children}
      {!loading && iconRight && <span className="fx-button__icon" aria-hidden="true">{iconRight}</span>}
    </button>
  );
});
