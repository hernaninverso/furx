// Audit sync endpoint — desktop POSTs replayable events here. Council K
// 3/3 must-fix: unauthenticated POST was the original gap. This route now
// requires the HMAC signature from web-companion/lib/auth.ts.
//
// In production the companion runs against a real DB (KV/D1/etc); this
// scaffolding stores in-memory so a fresh deploy starts empty.

import { NextRequest, NextResponse } from "next/server";
import { verifyAudit } from "../../../lib/auth";

interface AuditEvent {
  id: string;
  install_id: string;
  at: string;
  kind: string;
  actor: string;
  payload: unknown;
}

// In-memory store for scaffolding. Replace with KV / D1 / Postgres for prod.
// Per-install cap: 10k events; LRU eviction.
const STORE: Map<string, AuditEvent[]> = new Map();
const PER_INSTALL_CAP = 10_000;

function recordEvent(installId: string, ev: AuditEvent) {
  const list = STORE.get(installId) ?? [];
  list.push(ev);
  if (list.length > PER_INSTALL_CAP) list.splice(0, list.length - PER_INSTALL_CAP);
  STORE.set(installId, list);
}

// Audit Codex MED: in-memory STORE is shared across requests ONLY in long-
// running runtimes (node, edge with persistent isolate). On Vercel/Cloudflare
// serverless cold starts, this resets. The companion is scaffolding-only —
// gate behind FURX_COMPANION_DEV unless a real KV/D1/Postgres backend is
// wired. Hard-fail in prod so nobody assumes a working sync.
function envIsDev(): boolean {
  return process.env.FURX_COMPANION_DEV === "true" || process.env.NODE_ENV !== "production";
}

const MAX_BODY_BYTES = 256 * 1024;

export async function POST(req: NextRequest) {
  if (!envIsDev()) {
    return NextResponse.json({ error: "companion sync requires persistent backend (set FURX_COMPANION_DEV=true for in-memory dev mode, or wire KV/D1)" }, { status: 503 });
  }
  // Audit Codex MED: reject by Content-Length BEFORE awaiting req.text() so
  // a malicious POST can't allocate 1GB just to be rejected post-read.
  const declared = parseInt(req.headers.get("content-length") ?? "0", 10);
  if (Number.isFinite(declared) && declared > MAX_BODY_BYTES) {
    return NextResponse.json({ error: "payload too large (content-length)" }, { status: 413 });
  }
  const raw = await req.text();
  // Secondary guard for chunked transfers that don't declare Content-Length.
  if (raw.length > MAX_BODY_BYTES) {
    return NextResponse.json({ error: "payload too large" }, { status: 413 });
  }
  let body: { install_id?: string; ts?: number; sig?: string; events?: AuditEvent[] };
  try { body = JSON.parse(raw); } catch { return NextResponse.json({ error: "bad json" }, { status: 400 }); }

  const installId = String(body.install_id ?? "");
  const ts = typeof body.ts === "number" ? body.ts : NaN;
  const sig = String(body.sig ?? "");
  const events = Array.isArray(body.events) ? body.events : [];
  if (!installId || events.length === 0) {
    return NextResponse.json({ error: "missing install_id or events" }, { status: 400 });
  }
  if (events.length > 1000) {
    return NextResponse.json({ error: "too many events per batch" }, { status: 413 });
  }

  // Secret resolution: per-install in env (PROD) or a dev-only fallback for
  // local scaffolding. Companion deploy MUST set per-install secrets in
  // env or KV — the dev fallback is intentionally weak so it never ships.
  const secret = process.env[`FURX_INSTALL_SECRET_${installId.replace(/[^A-Z0-9_]/gi, "_").toUpperCase()}`]
    ?? process.env.FURX_DEV_SECRET;
  if (!secret) {
    return NextResponse.json({ error: "install not paired" }, { status: 401 });
  }

  // Sign over the EVENTS array (not the whole envelope) so the desktop only
  // hashes the data it owns; install_id and ts are validated against the sig.
  const eventsJson = JSON.stringify(events);
  const ok = await verifyAudit(installId, secret, eventsJson, ts, sig);
  if (!ok) {
    return NextResponse.json({ error: "invalid signature" }, { status: 401 });
  }

  // Ultra-review codex MED: validate each event has the required shape AND
  // refuses to record an event whose install_id doesn't match the envelope
  // (would have been ignored anyway, but better to surface the mismatch).
  // Also dedupe by (install_id, ev.id) — Idempotent retries from the desktop
  // are safe.
  const existing = new Set((STORE.get(installId) ?? []).map((e) => e.id));
  let accepted = 0;
  let rejected = 0;
  let duped = 0;
  for (const ev of events) {
    if (!ev || typeof ev !== "object" || typeof ev.id !== "string"
        || typeof ev.at !== "string" || typeof ev.kind !== "string") {
      rejected += 1; continue;
    }
    if (ev.install_id && ev.install_id !== installId) {
      rejected += 1; continue;
    }
    if (existing.has(ev.id)) { duped += 1; continue; }
    existing.add(ev.id);
    recordEvent(installId, { ...ev, install_id: installId });
    accepted += 1;
  }
  return NextResponse.json({
    accepted, rejected, duped,
    total: STORE.get(installId)?.length ?? 0,
  });
}

export async function GET(req: NextRequest) {
  if (!envIsDev()) {
    return NextResponse.json({ error: "companion sync requires persistent backend (set FURX_COMPANION_DEV=true for in-memory dev mode, or wire KV/D1)" }, { status: 503 });
  }
  // Read-only audit replay endpoint. Companion mobile / web reads its own
  // events with the same HMAC scheme. We send `ts` as a query param plus
  // `sig` over the install_id alone so a GET can be made without a body.
  const url = new URL(req.url);
  const installId = url.searchParams.get("install_id") ?? "";
  const ts = parseInt(url.searchParams.get("ts") ?? "0", 10);
  const sig = url.searchParams.get("sig") ?? "";

  const secret = process.env[`FURX_INSTALL_SECRET_${installId.replace(/[^A-Z0-9_]/gi, "_").toUpperCase()}`]
    ?? process.env.FURX_DEV_SECRET;
  if (!secret) {
    return NextResponse.json({ error: "install not paired" }, { status: 401 });
  }

  const ok = await verifyAudit(installId, secret, "READ", ts, sig);
  if (!ok) {
    return NextResponse.json({ error: "invalid signature" }, { status: 401 });
  }

  const events = STORE.get(installId) ?? [];
  return NextResponse.json({ install_id: installId, count: events.length, events });
}
