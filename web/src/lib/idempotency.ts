// web/src/lib/idempotency.ts — 050 Ola 8 P2 (FR-004).
//
// Genera una idempotency key estable por acción de UI. Defensa extra sobre el `invokeSeqRef` de la
// Ola 3 (que cubre la doble-respuesta INTRA-ventana): la key cubre el caso multi-instancia FUTURO —
// dos ventanas/instancias de Furx que replayen la MISMA decisión de card → el backend hace no-op
// idempotente al ver la misma key. Charset alfanumérico + `-`/`_` (el backend valida el mismo set y
// capea a 128 chars), así que `crypto.randomUUID()` (hex + guiones) es válido tal cual.

/// Fuente de UUID inyectable (solo para tests — `globalThis.crypto` es un getter de solo-lectura en
/// Node, no se puede stubear). En producción se toma `globalThis.crypto`.
interface UuidSource {
  randomUUID?: () => string;
}

/// Crea una key única para una acción (UUID v4 si hay crypto; fallback determinista por timestamp +
/// random si no). El llamador la genera UNA vez por decisión y la reusa en los retries (así un retry
/// del guard NO genera una key nueva → la idempotencia se mantiene). `src` permite inyectar la fuente
/// en tests; por defecto usa `globalThis.crypto`.
export function makeIdempotencyKey(src?: UuidSource): string {
  const c = src ?? (globalThis as { crypto?: Crypto }).crypto;
  if (c && typeof c.randomUUID === "function") {
    return c.randomUUID();
  }
  // Fallback (entornos sin crypto.randomUUID): timestamp + random base36. Sigue el charset válido.
  const rand = Math.random().toString(36).slice(2, 12);
  return `k-${Date.now().toString(36)}-${rand}`;
}
