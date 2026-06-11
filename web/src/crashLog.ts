// C2 — Frontend → backend crash relay. Captures JS errors and unhandled
// promise rejections, then dispatches to the Rust crash_log_js command.
//
// Strict guards keep us well-behaved under cascading failures:
// - Max 30 reports/min (matches backend rate limit)
// - Each payload truncated to MAX_PAYLOAD_BYTES
// - Best-effort: any throw inside the handler is swallowed
// - Re-entrancy guard so logging-a-crash can't itself crash

import { invoke } from "@tauri-apps/api/core";

const MAX_PAYLOAD_BYTES = 8 * 1024;
const RATE_LIMIT_PER_MINUTE = 30;
let rateCount = 0;
let rateWindowStart = Date.now();
let reentrant = false;
let installed = false;

interface JsCrashPayload {
  source: "js-error" | "js-unhandled-rejection" | "manual";
  message: string;
  location?: string;
  stack?: string;
}

// Codex MED v1: truncate by UTF-8 byte length, not UTF-16 codepoints, and account
// for the suffix so the final byte-length stays ≤ max.
const TRUNC_SUFFIX = "…[truncated]";
const TRUNC_SUFFIX_BYTES = new TextEncoder().encode(TRUNC_SUFFIX).length;

function truncate(s: string | undefined, max: number): string | undefined {
  if (!s) return undefined;
  const encoder = new TextEncoder();
  const bytes = encoder.encode(s);
  if (bytes.length <= max) return s;
  const budget = Math.max(0, max - TRUNC_SUFFIX_BYTES);
  // Codex MED v2: TextDecoder with fatal:false inserts U+FFFD for partial UTF-8
  // sequences, adding 3 bytes that can push us over `max`. Use fatal:true on a
  // strict slice; on error, walk back to the last clean UTF-8 boundary.
  const strict = new TextDecoder("utf-8", { ignoreBOM: true, fatal: true });
  let end = Math.min(budget, bytes.length);
  for (;;) {
    try {
      const head = strict.decode(bytes.subarray(0, end));
      return head + TRUNC_SUFFIX;
    } catch {
      if (end === 0) return TRUNC_SUFFIX;
      end -= 1;
    }
  }
}

function rateLimitOk(): boolean {
  const now = Date.now();
  if (now - rateWindowStart >= 60_000) {
    rateWindowStart = now;
    rateCount = 0;
  }
  if (rateCount >= RATE_LIMIT_PER_MINUTE) return false;
  rateCount += 1;
  return true;
}

export function reportCrash(payload: JsCrashPayload): void {
  // Codex LOW v1: sync reentry guard scoped to PAYLOAD BUILDING only. The async
  // invoke is fire-and-forget so a slow/hung backend doesn't drop unrelated reports.
  if (reentrant) return;
  if (!rateLimitOk()) return;
  reentrant = true;
  let body: Record<string, unknown>;
  try {
    body = {
      source: payload.source,
      message: truncate(payload.message, MAX_PAYLOAD_BYTES) ?? "",
      location: truncate(payload.location, 512),
      stack: truncate(payload.stack, MAX_PAYLOAD_BYTES),
    };
  } catch (e) {
    reentrant = false;
    console.error("crash_log payload build failed", e);
    return;
  }
  reentrant = false;
  void invoke("crash_log_js", { payload: body }).catch((e) => {
    console.error("crash_log_js failed", e);
  });
}

export function installCrashHandlers(): void {
  if (installed) return;
  installed = true;
  if (typeof window === "undefined") return;

  window.addEventListener("error", (ev) => {
    const err = ev.error;
    const message = (err && typeof err === "object" && "message" in err)
      ? String((err as Error).message)
      : String(ev.message ?? "unknown error");
    const stack = (err && typeof err === "object" && "stack" in err)
      ? String((err as Error).stack ?? "")
      : undefined;
    const location = ev.filename
      ? `${ev.filename}:${ev.lineno ?? 0}:${ev.colno ?? 0}`
      : undefined;
    reportCrash({ source: "js-error", message, stack, location });
  });

  window.addEventListener("unhandledrejection", (ev) => {
    const reason = ev.reason;
    let message = "unhandled rejection";
    let stack: string | undefined;
    if (reason instanceof Error) {
      message = reason.message;
      stack = reason.stack;
    } else if (reason != null) {
      try { message = typeof reason === "string" ? reason : JSON.stringify(reason); }
      catch { message = String(reason); }
    }
    reportCrash({ source: "js-unhandled-rejection", message, stack });
  });
}

export const __crashLogTest__ = { rateLimitOk, truncate, MAX_PAYLOAD_BYTES, RATE_LIMIT_PER_MINUTE };
