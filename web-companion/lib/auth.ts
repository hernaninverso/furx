// Furx Web Companion · HMAC pairing helper.
//
// Council K must-fix (3/3): the original audit endpoint had no auth — any
// caller could POST events. This module is the shared validator: every audit
// upload from the desktop carries Authorization: Bearer <token>, where the
// token is a sha256 of "install_id|secret|epoch_minute". The companion
// regenerates the same string with its stored secret and compares constant-time.
//
// `secret` is the 64-hex string you paste from desktop Settings → Mobile.
// `install_id` is a UUID the desktop generates on first launch.

const ENC = new TextEncoder();

/// Canonical bytes for HMAC. Audit Codex MED: the previous version used
/// `string.length` (UTF-16 code units) — incompatible with any consumer
/// expecting UTF-8 byte length. Now: encode each field as UTF-8 bytes, prefix
/// each with the byte length as a 0-padded 10-char decimal, separator-free.
/// This is the WEB canonical — distinct (and intentionally separate) from
/// the desktop's `mobile_bridge::canonical_bytes()` which serves a different
/// transport (WebSocket binary).
function canonical(installId: string, ts: number, body: string): Uint8Array {
  const tag = ENC.encode("WebAudit");
  const idBytes = ENC.encode(installId);
  const tsBytes = ENC.encode(String(ts));
  const bodyBytes = ENC.encode(body);
  const lenPrefix = (n: number) => ENC.encode(n.toString(10).padStart(10, "0"));
  const idLen = lenPrefix(idBytes.length);
  const tsLen = lenPrefix(tsBytes.length);
  const bdLen = lenPrefix(bodyBytes.length);
  const total = tag.length + idLen.length + idBytes.length
              + tsLen.length + tsBytes.length
              + bdLen.length + bodyBytes.length;
  const buf = new Uint8Array(total);
  let off = 0;
  const push = (b: Uint8Array) => { buf.set(b, off); off += b.length; };
  push(tag); push(idLen); push(idBytes);
  push(tsLen); push(tsBytes);
  push(bdLen); push(bodyBytes);
  return buf;
}

async function hmacSha256Bytes(secret: string, msg: Uint8Array): Promise<string> {
  // Ultra-review distribution HIGH: explicit `extractable: false` so the key
  // material can never be pulled back out via crypto.subtle.exportKey, even if
  // a future caller of this helper accidentally tries.
  // Ultra-review code-quality HIGH: try/catch so SubtleCrypto failure surfaces
  // a clear error instead of an unhandled rejection on the next await chain.
  try {
    const key = await crypto.subtle.importKey(
      "raw", ENC.encode(secret) as BufferSource,
      { name: "HMAC", hash: "SHA-256" },
      false /* extractable */,
      ["sign"],
    );
    const buf = await crypto.subtle.sign("HMAC", key, msg as BufferSource);
    return Array.from(new Uint8Array(buf))
      .map((b) => b.toString(16).padStart(2, "0"))
      .join("");
  } catch (e) {
    throw new Error(`HMAC unavailable (SubtleCrypto failed): ${e instanceof Error ? e.message : String(e)}`);
  }
}

export async function signAudit(installId: string, secret: string, body: string): Promise<{ ts: number; sig: string }> {
  const ts = Math.floor(Date.now() / 1000);
  const sig = await hmacSha256Bytes(secret, canonical(installId, ts, body));
  return { ts, sig };
}

export async function verifyAudit(
  installId: string,
  secret: string,
  body: string,
  ts: number,
  providedSig: string,
): Promise<boolean> {
  // Defence-in-depth bounds: reject ridiculous payloads, stale ts (>5min
  // skew), non-hex sig — before even doing the HMAC.
  if (!installId || installId.length > 128) return false;
  if (!Number.isFinite(ts)) return false;
  const now = Math.floor(Date.now() / 1000);
  if (Math.abs(now - ts) > 300) return false;
  if (typeof providedSig !== "string" || providedSig.length !== 64) return false;
  if (!/^[0-9a-f]+$/.test(providedSig)) return false;
  if (typeof body !== "string" || body.length > 1024 * 1024) return false;

  const expected = await hmacSha256Bytes(secret, canonical(installId, ts, body));
  // Constant-time compare — small variant since SubtleCrypto doesn't expose one.
  if (expected.length !== providedSig.length) return false;
  let diff = 0;
  for (let i = 0; i < expected.length; i++) {
    diff |= expected.charCodeAt(i) ^ providedSig.charCodeAt(i);
  }
  return diff === 0;
}
