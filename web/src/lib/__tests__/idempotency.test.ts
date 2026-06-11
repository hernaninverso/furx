// 050 Ola 8 P2 (FR-004) — tests de la generación de idempotency key (lógica pura).
import { makeIdempotencyKey } from "../idempotency.ts";

let pass = 0, fail = 0;
function ok(cond: boolean, name: string) { if (cond) pass++; else { fail++; console.log(`FAIL ${name}`); } }

// El charset debe coincidir con el que valida el backend: [A-Za-z0-9_-], cap 128.
const VALID = /^[A-Za-z0-9_-]+$/;

{
  const k = makeIdempotencyKey();
  ok(typeof k === "string" && k.length > 0, "genera un string no vacío");
  ok(k.length <= 128, "no excede el cap de 128 del backend");
  ok(VALID.test(k), `usa solo el charset válido del backend (got: ${k})`);
}

// Unicidad: dos llamadas sucesivas no colisionan (UUID v4 o fallback random).
{
  const seen = new Set<string>();
  for (let i = 0; i < 1000; i++) seen.add(makeIdempotencyKey());
  ok(seen.size === 1000, "1000 keys son todas únicas");
}

// Fallback determinista (`makeIdempotencyKeyFallback`): producir una key válida del charset SIN
// depender de crypto.randomUUID (que en Node es un getter de solo-lectura, no stubeable).
{
  const k = makeIdempotencyKey({ randomUUID: undefined });
  ok(VALID.test(k) && k.length > 0 && k.length <= 128, `fallback produce key válida (got: ${k})`);
  ok(k.startsWith("k-"), "el fallback usa el prefijo 'k-'");
}

console.log(`idempotency: ${pass} passed, ${fail} failed`);
process.exit(fail === 0 ? 0 : 1);
