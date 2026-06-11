// 022 US9 (FR-014) — Lógica pura del design-system <Button>.
// ──────────────────────────────────────────────────────────────────────────
// Escala CERRADA de variantes semánticas y tamaños. Esta es la fuente de verdad
// del mapeo variant/size/estado → clases CSS (`.fx-button*`). El componente
// `components/Button.tsx` la consume; el test `__tests__/buttonVariants.test.ts`
// la valida (variantes cerradas + un único radio/altura por size).
//
// Cero color/radio/altura hardcodeado acá: las clases resuelven SOLO contra los
// tokens semánticos V3 (`web/src/styles/buttonComponent.css`). Esto es lo que
// mata las "formas caprichosas": un único radio y una única altura por size.
// `node --experimental-strip-types` lo puede importar (sin JSX).

/** Variantes semánticas — escala CERRADA. No agregar formas ad-hoc. */
export const BUTTON_VARIANTS = ["primary", "secondary", "danger", "ghost", "success"] as const;
export type ButtonVariant = (typeof BUTTON_VARIANTS)[number];

/** Tamaños — escala CERRADA. Un único radio/altura por size. */
export const BUTTON_SIZES = ["sm", "md", "lg"] as const;
export type ButtonSize = (typeof BUTTON_SIZES)[number];

/** Mapeo legacy `className` ad-hoc → variante/size canónico (para la migración). */
export const LEGACY_BUTTON_MAP: Record<string, { variant: ButtonVariant; size?: ButtonSize }> = {
  primary: { variant: "primary" },
  ghost: { variant: "ghost" },
  danger: { variant: "danger" },
  secondary: { variant: "secondary" },
  success: { variant: "success" },
  mini: { variant: "secondary", size: "sm" },
  "mini primary": { variant: "primary", size: "sm" },
  "mini danger": { variant: "danger", size: "sm" },
};

function isVariant(v: string): v is ButtonVariant {
  return (BUTTON_VARIANTS as readonly string[]).includes(v);
}
function isSize(s: string): s is ButtonSize {
  return (BUTTON_SIZES as readonly string[]).includes(s);
}

export interface ButtonClassInput {
  variant?: ButtonVariant;
  size?: ButtonSize;
  loading?: boolean;
  /** clase de escape opcional para casos legítimos (layout, márgenes). */
  className?: string;
}

/**
 * Devuelve la cadena de clases canónicas para un botón. Solo emite clases del
 * design-system (`fx-button*`); cualquier `className` extra del caller se anexa
 * al final (escape documentado para layout, nunca para color/forma).
 */
export function buttonClasses(input: ButtonClassInput = {}): string {
  const variant: ButtonVariant = input.variant && isVariant(input.variant) ? input.variant : "secondary";
  const size: ButtonSize = input.size && isSize(input.size) ? input.size : "md";
  const cls = ["fx-button", `fx-button--${variant}`, `fx-button--${size}`];
  if (input.loading) cls.push("fx-button--loading");
  if (input.className) cls.push(input.className.trim());
  return cls.join(" ");
}
