// 019 F2 T021 — `FormFromSchema`: separación EXPLÍCITA en 3 capas (FR-010, anti command-injection).
//
//   1) RENDERING       — `FieldSpec[]` describe QUÉ campos pintar. Sólo data; cero ejecución.
//   2) VALIDATION      — `validate(schema, values)` valida/sanea el input contra el schema.
//                        Pura, centralizada, idéntica para toda superficie UI-driven.
//   3) EXECUTION-POLICY — `ExecutionPolicy` decide CÓMO se ejecuta una vez validado. El form NUNCA
//                        ejecuta: produce un `ValidatedInput` opaco y DELEGA en la policy. Así el
//                        input del usuario JAMÁS se concatena a un comando/shell sin pasar por el
//                        gate de validación + por una policy con allow-list de comandos.
//
// El invariante de seguridad: no se puede llegar a `policy.execute(...)` sin un `ValidatedInput`, y
// `ValidatedInput` SÓLO lo emite `validate()` cuando no hay errores. El brand `__validated` hace que
// el compilador rechace pasar valores crudos a la policy.

export type FieldKind = "text" | "number" | "select" | "boolean" | "textarea";

export interface FieldSpec {
  /** clave del valor (id estable). */
  name: string;
  label: string;
  kind: FieldKind;
  required?: boolean;
  placeholder?: string;
  /** select: opciones permitidas (allow-list → el valor debe ser una de éstas). */
  options?: { value: string; label: string }[];
  /** text/textarea: longitud máxima (default 4096). */
  maxLen?: number;
  /** number: rango inclusivo. */
  min?: number;
  max?: number;
  /**
   * text: patrón permitido. RESTRINGE: el valor debe matchear el pattern ADEMÁS de pasar el
   * deny-list anti-shell (`SHELL_META`). El pattern NUNCA relaja el anti-shell (audit HIGH 2).
   */
  pattern?: RegExp;
  /**
   * Opt-out EXPLÍCITO y muy visible del deny-list anti-shell para campos que legítimamente
   * necesitan un metacaracter (default `false`). REQUIERE un `pattern` (RegExp) acotado: si está en
   * `true` SIN `pattern`, la validación FALLA con un error de configuración del schema (audit ronda 2
   * H2) — así nunca queda "todo abierto". Para permitir un metacaracter hay que proveer un pattern
   * restrictivo que acote el input. Default = anti-shell SIEMPRE activo.
   */
  allowShellMeta?: boolean;
}

export interface FormSchema {
  /** id del comando del registry que esta forma va a invocar (la policy lo valida contra su allow-list). */
  commandId: string;
  fields: FieldSpec[];
}

export type FormValues = Record<string, string | number | boolean | null>;

export interface ValidationResult {
  ok: boolean;
  /** errores por campo (name → mensaje). vacío si ok. */
  errors: Record<string, string>;
  /** sólo presente si ok: payload saneado y "branded" listo para la policy. */
  validated?: ValidatedInput;
}

/** Caracteres de shell peligrosos para campos de texto libre que NO usan un pattern propio. */
// eslint-disable-next-line no-control-regex
const SHELL_META = /[`$;&|<>(){}\\\n\r\x00]/;

/**
 * Branded type: un payload que PASÓ validación. La marca NO vive en el objeto (un Symbol-en-el-objeto
 * es falsificable: `Object.getOwnPropertySymbols()` lo extrae de un valor real, o un `as ValidatedInput`
 * lo evade en runtime — audit HIGH 1). En su lugar usamos un WeakSet PRIVADO del módulo como registro
 * de marca OPACA: `validate()` registra el objeto resultante; `isValidated()`/`executeWithPolicy()`
 * chequean pertenencia. El WeakSet NO se exporta y no hay método de inserción público → ningún código
 * externo puede registrar un objeto, así que la marca es INFALSIFICABLE en runtime.
 */
export interface ValidatedInput {
  readonly commandId: string;
  readonly values: Readonly<FormValues>;
}

/**
 * Registro de marca opaca. Privado al módulo: la ÚNICA forma de agregar un objeto es `brand()`, que
 * sólo invoca `validate()`. No se exporta ni se expone ningún método de inserción → infalsificable.
 */
const VALIDATED_REGISTRY = new WeakSet<object>();

function brand(commandId: string, values: FormValues): ValidatedInput {
  // Congelamos (Object.freeze) el objeto validado para que su contenido sea inmutable tras la marca.
  const input: ValidatedInput = Object.freeze({ commandId, values: Object.freeze({ ...values }) });
  VALIDATED_REGISTRY.add(input);
  return input;
}

/** True sólo para objetos emitidos por `validate()` (registrados en el WeakSet privado). */
export function isValidated(x: unknown): x is ValidatedInput {
  return typeof x === "object" && x !== null && VALIDATED_REGISTRY.has(x as object);
}

/**
 * CAPA 2 — Validación pura. Recorre el schema, valida/sanea cada campo y, si TODO pasa, emite el
 * `ValidatedInput` branded. Determinística, sin efectos, sin red.
 */
export function validate(schema: FormSchema, values: FormValues): ValidationResult {
  const errors: Record<string, string> = {};
  const clean: FormValues = {};

  for (const f of schema.fields) {
    const raw = values[f.name];

    // requerido
    const empty = raw == null || raw === "" || (f.kind === "select" && raw === "");
    if (f.required && empty) {
      errors[f.name] = "Requerido.";
      continue;
    }
    if (empty) {
      clean[f.name] = f.kind === "boolean" ? false : null;
      continue;
    }

    switch (f.kind) {
      case "boolean":
        clean[f.name] = raw === true || raw === "true";
        break;
      case "number": {
        const n = typeof raw === "number" ? raw : Number(String(raw).trim());
        if (!Number.isFinite(n)) { errors[f.name] = "Debe ser un número."; break; }
        if (f.min != null && n < f.min) { errors[f.name] = `Mínimo ${f.min}.`; break; }
        if (f.max != null && n > f.max) { errors[f.name] = `Máximo ${f.max}.`; break; }
        clean[f.name] = n;
        break;
      }
      case "select": {
        const v = String(raw);
        const allowed = (f.options ?? []).map((o) => o.value);
        if (!allowed.includes(v)) { errors[f.name] = "Opción no permitida."; break; }
        clean[f.name] = v;
        break;
      }
      case "text":
      case "textarea": {
        const v = String(raw);
        const max = f.maxLen ?? 4096;
        // CONFIG GUARD (audit ronda 2 H2): `allowShellMeta:true` desactiva el deny-list anti-shell;
        // exigir un `pattern` que ACOTE el input es obligatorio, sino el campo queda "todo abierto".
        // Sin pattern → error de configuración del schema (se aplica ANTES de validar el valor).
        if (f.allowShellMeta && !f.pattern) {
          errors[f.name] = "Config inválida: allowShellMeta requiere un pattern explícito que acote el input.";
          break;
        }
        if (v.length > max) { errors[f.name] = `Máximo ${max} caracteres.`; break; }
        // ANTI-SHELL SIEMPRE (audit HIGH 2): el deny-list se aplica salvo opt-out explícito, AUNQUE
        // haya `pattern`. Un `pattern: /.*/` ya NO relaja el anti-shell — el pattern sólo RESTRINGE más.
        if (!f.allowShellMeta && SHELL_META.test(v)) {
          errors[f.name] = "Contiene caracteres no permitidos.";
          break;
        }
        // El `pattern` RESTRINGE adicionalmente (nunca relaja): el valor debe matchearlo ADEMÁS.
        if (f.pattern && !f.pattern.test(v)) {
          errors[f.name] = "Formato inválido.";
          break;
        }
        clean[f.name] = v;
        break;
      }
    }
  }

  const ok = Object.keys(errors).length === 0;
  return ok ? { ok, errors, validated: brand(schema.commandId, clean) } : { ok, errors };
}

/**
 * CAPA 3 — Execution-policy. El form NO ejecuta; entrega el `ValidatedInput` a una policy que:
 *   - chequea que el comando esté en su ALLOW-LIST (RBAC/capacidad), y
 *   - ejecuta vía un `runner` inyectado (en prod = `invoke` del gate universal → audit + aprobación).
 * Cualquier intento de ejecutar input no-validado lanza (defensa en profundidad).
 */
export interface ExecutionPolicy {
  /** comandos que esta policy autoriza a ejecutar (allow-list). */
  allow: readonly string[];
  /** ejecutor real (en la app: `invoke` gobernado). */
  runner: (commandId: string, values: Readonly<FormValues>) => Promise<unknown>;
}

export async function executeWithPolicy(
  policy: ExecutionPolicy,
  input: ValidatedInput,
): Promise<unknown> {
  if (!isValidated(input)) {
    throw new Error("execution rechazada: input no validado");
  }
  if (!policy.allow.includes(input.commandId)) {
    throw new Error(`comando no autorizado por la policy: ${input.commandId}`);
  }
  return policy.runner(input.commandId, input.values);
}
