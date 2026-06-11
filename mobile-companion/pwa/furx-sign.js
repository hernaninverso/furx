// furx-sign.js — client-side HMAC signing for the Furx Mobile bridge protocol.
//
// MUST byte-for-byte match the Rust `canonical_bytes` + `verify_hmac` in
// src-tauri/src/services/mobile_bridge.rs, or every frame is rejected.
//
// Canonical layout (length-prefixed, unambiguous):
//   tag(8 ASCII bytes) || for each field: [u64 LE length || raw bytes]
// Fields in order: nonce, ts, scope, source, body.
//   - ts is the i64 little-endian (8 bytes) THEN length-prefixed like any field
//     (so it contributes: u64 LE len=8, then 8 bytes).
// HMAC key = the UTF-8 bytes of the 64-char hex secret STRING (NOT decoded hex)
//   — Rust uses `secret.as_bytes()` on the hex string.
//
// HMAC is implemented in PURE JS (not Web Crypto `crypto.subtle`) ON PURPOSE:
// `crypto.subtle` only exists in a SECURE CONTEXT (https or localhost). A real
// phone loads this PWA from the desktop's LAN/tailnet IP over plain http — an
// INSECURE origin — where `crypto.subtle` is undefined. A pure-JS HMAC-SHA256
// works in any context, so pairing/signing works from any phone while the bridge
// stays plaintext WS (encryption from loopback / Tailscale WireGuard, MC-2).
// Payloads are tiny, so the perf cost is irrelevant.

const TAGS = {
  hello: "HelloMsg",
  subscribe: "Subscrib",
  pty_write: "PtyWrite",
  approve_tool_call: "ApprovTC",
  // 017 — execute a registry command by id ref (command_id in the scope slot).
  execute_command: "ExecCmd_",
};

// 017 — 8-byte tags for the SIGNED server→client frames (defense-in-depth). Must
// match TAG_NAVSPEC/TAG_CMDCATALOG/TAG_APPEVENT in mobile_bridge.rs.
export const OUTBOUND_TAGS = {
  nav_spec: "NavSpec_",
  command_catalog: "CmdCatlg",
  app_event: "AppEvnt_",
};

function u64le(n) {
  const buf = new ArrayBuffer(8);
  new DataView(buf).setBigUint64(0, BigInt(n), true);
  return new Uint8Array(buf);
}

function i64le(n) {
  const buf = new ArrayBuffer(8);
  new DataView(buf).setBigInt64(0, BigInt(n), true);
  return new Uint8Array(buf);
}

function concatBytes(chunks) {
  const total = chunks.reduce((a, c) => a + c.length, 0);
  const out = new Uint8Array(total);
  let off = 0;
  for (const c of chunks) {
    out.set(c, off);
    off += c.length;
  }
  return out;
}

// Build the canonical byte string for one frame.
// tag: 8-char ASCII string. ts: integer. others: strings.
export function canonicalBytes(tag, nonce, ts, scope, source, body) {
  const enc = new TextEncoder();
  const chunks = [];
  chunks.push(enc.encode(tag)); // exactly 8 bytes
  const pushField = (bytes) => {
    chunks.push(u64le(bytes.length));
    chunks.push(bytes);
  };
  pushField(enc.encode(nonce));
  pushField(i64le(ts)); // i64 LE, length-prefixed (len = 8)
  pushField(enc.encode(scope));
  pushField(enc.encode(source));
  pushField(enc.encode(body));
  return concatBytes(chunks);
}

function toHex(bytes) {
  return Array.from(bytes)
    .map((b) => b.toString(16).padStart(2, "0"))
    .join("");
}

// ── Pure-JS SHA-256 (FIPS 180-4), operating on Uint8Array → 32-byte digest ──
const K256 = new Uint32Array([
  0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
  0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
  0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
  0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
  0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
  0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
  0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
  0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
]);

function sha256(msg) {
  const h = new Uint32Array([
    0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19,
  ]);
  const ml = msg.length;
  const withOne = ml + 1;
  const k = (56 - (withOne % 64) + 64) % 64;
  const total = withOne + k + 8;
  const buf = new Uint8Array(total);
  buf.set(msg);
  buf[ml] = 0x80;
  const bitLen = ml * 8;
  // 64-bit big-endian length (high 32 bits assumed 0 for our sizes).
  const dv = new DataView(buf.buffer);
  dv.setUint32(total - 4, bitLen >>> 0, false);
  dv.setUint32(total - 8, Math.floor(bitLen / 0x100000000) >>> 0, false);

  const w = new Uint32Array(64);
  const rotr = (x, n) => (x >>> n) | (x << (32 - n));
  for (let off = 0; off < total; off += 64) {
    for (let i = 0; i < 16; i++) w[i] = dv.getUint32(off + i * 4, false);
    for (let i = 16; i < 64; i++) {
      const s0 = rotr(w[i - 15], 7) ^ rotr(w[i - 15], 18) ^ (w[i - 15] >>> 3);
      const s1 = rotr(w[i - 2], 17) ^ rotr(w[i - 2], 19) ^ (w[i - 2] >>> 10);
      w[i] = (w[i - 16] + s0 + w[i - 7] + s1) >>> 0;
    }
    let [a, b, c, d, e, f, g, hh] = h;
    for (let i = 0; i < 64; i++) {
      const S1 = rotr(e, 6) ^ rotr(e, 11) ^ rotr(e, 25);
      const ch = (e & f) ^ (~e & g);
      const t1 = (hh + S1 + ch + K256[i] + w[i]) >>> 0;
      const S0 = rotr(a, 2) ^ rotr(a, 13) ^ rotr(a, 22);
      const maj = (a & b) ^ (a & c) ^ (b & c);
      const t2 = (S0 + maj) >>> 0;
      hh = g; g = f; f = e; e = (d + t1) >>> 0; d = c; c = b; b = a; a = (t1 + t2) >>> 0;
    }
    h[0] = (h[0] + a) >>> 0; h[1] = (h[1] + b) >>> 0; h[2] = (h[2] + c) >>> 0; h[3] = (h[3] + d) >>> 0;
    h[4] = (h[4] + e) >>> 0; h[5] = (h[5] + f) >>> 0; h[6] = (h[6] + g) >>> 0; h[7] = (h[7] + hh) >>> 0;
  }
  const out = new Uint8Array(32);
  const odv = new DataView(out.buffer);
  for (let i = 0; i < 8; i++) odv.setUint32(i * 4, h[i], false);
  return out;
}

// HMAC-SHA256 per RFC 2104. key + message are Uint8Array → 32-byte digest.
function hmacSha256(keyBytes, msg) {
  const BLOCK = 64;
  let key = keyBytes;
  if (key.length > BLOCK) key = sha256(key);
  const k = new Uint8Array(BLOCK); // zero-padded
  k.set(key);
  const ipad = new Uint8Array(BLOCK + msg.length);
  const opad = new Uint8Array(BLOCK + 32);
  for (let i = 0; i < BLOCK; i++) {
    ipad[i] = k[i] ^ 0x36;
    opad[i] = k[i] ^ 0x5c;
  }
  ipad.set(msg, BLOCK);
  const inner = sha256(ipad);
  opad.set(inner, BLOCK);
  return sha256(opad);
}

// Compute the hex HMAC-SHA256 sig over canonical bytes. secretHex is the
// 64-char hex string shown in Settings -> Mobile (used as UTF-8 key bytes).
// Returns synchronously, but stays `async` so callers needn't change.
export async function hmacHex(secretHex, canonical) {
  const keyBytes = new TextEncoder().encode(secretHex);
  return toHex(hmacSha256(keyBytes, canonical));
}

// Monotonic-ish nonce: time + random, unique per frame within the 60s window.
export function makeNonce() {
  const rand = crypto.getRandomValues(new Uint8Array(8));
  return `${Date.now().toString(36)}-${toHex(rand)}`;
}

export function nowSecs() {
  return Math.floor(Date.now() / 1000);
}

// Build a fully-signed frame object ready for JSON.stringify + ws.send.
//   kind: "hello" | "subscribe" | "pty_write" | "approve_tool_call"
//   fields: the type-specific scope/body values.
export async function signedFrame(secretHex, kind, fields) {
  const tag = TAGS[kind];
  if (!tag) throw new Error(`unknown signed frame kind: ${kind}`);
  const nonce = makeNonce();
  const ts = nowSecs();
  let scope = "";
  let source = "";
  let body = "";
  const base = { type: kind, nonce, ts };
  switch (kind) {
    case "hello":
      scope = fields.client_id;
      base.client_id = fields.client_id;
      break;
    case "subscribe":
      scope = fields.pane_id;
      base.pane_id = fields.pane_id;
      break;
    case "pty_write":
      scope = fields.pane_id;
      source = fields.source;
      body = fields.text;
      base.pane_id = fields.pane_id;
      base.source = fields.source;
      base.text = fields.text;
      break;
    case "approve_tool_call":
      scope = fields.correlation_id;
      body = fields.decision;
      base.correlation_id = fields.correlation_id;
      base.decision = fields.decision;
      break;
    case "execute_command":
      scope = fields.command_id;
      base.command_id = fields.command_id;
      break;
  }
  const canonical = canonicalBytes(tag, nonce, ts, scope, source, body);
  base.sig = await hmacHex(secretHex, canonical);
  return base;
}

// 017 [MF-2] — anti-replay for SIGNED server→client frames. A valid HMAC only
// proves authenticity; a genuine-but-stale frame could be replayed by a MITM on
// the Tailscale path (:43119, opt-in) and — after a reconnect floors the seq
// cursor to 0 — pass the `seq > lastSeq` check. Mirror the bridge's inbound
// guards: reject frames outside a time window and dedupe nonces (LRU). Nonces
// are inserted only AFTER the HMAC verifies, so a forged frame can't poison the
// cache. Legit re-sends on reconnect carry FRESH server nonces → no false reject.
const OUTBOUND_SKEW_SECS = 60;
const OUTBOUND_NONCE_CAP = 512;
const seenOutboundNonces = new Map(); // nonce -> 1 (Map preserves insertion order for LRU eviction)

function outboundNonceSeen(nonce) {
  if (seenOutboundNonces.has(nonce)) return true;
  seenOutboundNonces.set(nonce, 1);
  if (seenOutboundNonces.size > OUTBOUND_NONCE_CAP) {
    seenOutboundNonces.delete(seenOutboundNonces.keys().next().value); // evict oldest
  }
  return false;
}

// 017 [T063] — verify a SIGNED server→client frame (nav_spec / command_catalog /
// app_event). `body` is the JSON of the signed payload (for app_event it's the
// string "seq|json"). Recompute the canonical (tag in scope slot, empty src/body)
// and compare HMAC hex. Returns true iff the signature matches the pairing secret,
// the timestamp is within skew, and the nonce was not seen before (anti-replay).
// Constant-time-ish: full hex string compare (payloads are tiny, no secret leak).
export async function verifyOutbound(secretHex, kind, frame, body) {
  const tag = OUTBOUND_TAGS[kind];
  if (!tag) return false;
  if (typeof frame.nonce !== "string" || typeof frame.sig !== "string") return false;
  if (!Number.isFinite(frame.ts)) return false;
  // [MF-2a] reject stale/future frames (only a real threat on the Tailscale path).
  if (Math.abs(nowSecs() - frame.ts) > OUTBOUND_SKEW_SECS) return false;
  const canonical = canonicalBytes(tag, frame.nonce, frame.ts, body, "", "");
  const expect = await hmacHex(secretHex, canonical);
  // length-checked equality (hex strings, fixed length on match).
  if (expect.length !== frame.sig.length) return false;
  let diff = 0;
  for (let i = 0; i < expect.length; i++) diff |= expect.charCodeAt(i) ^ frame.sig.charCodeAt(i);
  if (diff !== 0) return false;
  // [MF-2b] HMAC OK → dedupe nonce (post-verify insert avoids cache poisoning by forged frames).
  if (outboundNonceSeen(frame.nonce)) return false;
  return true;
}
