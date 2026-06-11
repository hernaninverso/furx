/* CommandRow — fila de comando canónica para la futura ⌘K palette (US8, spec
 * 015). Icono · label · descripción · shortcut · badge de riesgo.
 *
 * El enum de riesgo refleja el Command Registry (US1): safe/destructive/
 * credential/external. Estados: hover, active (highlighted por teclado),
 * disabled. Sólo tokens.
 */

import { ReactNode } from "react";

/** Niveles de riesgo del Command Registry (US1, spec 015). */
export type CommandRisk = "safe" | "destructive" | "credential" | "external";

const RISK_LABEL: Record<CommandRisk, string> = {
  safe: "safe",
  destructive: "destruct",
  credential: "cred",
  external: "ext",
};

export interface CommandRowProps {
  /** Texto principal del comando. */
  label: ReactNode;
  /** Descripción / hint secundario (mono). */
  description?: ReactNode;
  /** Icono o glifo corto. */
  icon?: ReactNode;
  /** Teclas del shortcut, p.ej. ["⌘", "K"]. Se renderizan como <kbd>. */
  shortcut?: string[];
  /** Nivel de riesgo → badge. `safe` no muestra badge (ruido visual). */
  risk?: CommandRisk;
  /** Resaltado por navegación de teclado (no es :hover). */
  active?: boolean;
  /** Ejecutar el comando. */
  onSelect?: () => void;
  disabled?: boolean;
}

export function CommandRow({
  label,
  description,
  icon,
  shortcut,
  risk = "safe",
  active = false,
  onSelect,
  disabled = false,
}: CommandRowProps) {
  return (
    <button
      type="button"
      className="fxc-cmd-row"
      data-active={active ? "true" : undefined}
      aria-disabled={disabled || undefined}
      disabled={disabled}
      onClick={disabled ? undefined : onSelect}
    >
      {icon != null && (
        <span className="fxc-cmd-row__icon" aria-hidden="true">
          {icon}
        </span>
      )}
      <span className="fxc-cmd-row__main">
        <span className="fxc-cmd-row__label">{label}</span>
        {description != null && <span className="fxc-cmd-row__desc">{description}</span>}
      </span>
      {shortcut != null && shortcut.length > 0 && (
        <span className="fxc-cmd-row__shortcut" aria-hidden="true">
          {shortcut.map((k, i) => (
            <kbd key={i}>{k}</kbd>
          ))}
        </span>
      )}
      {risk !== "safe" && (
        <span className={`fxc-risk fxc-risk--${risk}`} title={`risk: ${risk}`}>
          {RISK_LABEL[risk]}
        </span>
      )}
    </button>
  );
}
