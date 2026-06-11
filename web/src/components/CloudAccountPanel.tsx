// Sprint #4 — Cloud Account panel for Furx Settings.
// Council 6/6 chose option A (custom URL scheme furx://) but for MVP we ship
// manual token paste as the backup path (V4 finding). Deep-link is the next step.
import { useEffect, useState } from "react";
import { invoke } from "../lib/invoke"; // 015 T015: invoke con flujo de aprobación universal

interface User { id: string; email: string; plan: "free" | "pro" | "team" | "ent"; }
interface UploaderStatus {
  state: 0 | 1 | 2;
  state_label: "idle" | "working" | "paused" | "unknown";
  dropped: number;
  succeeded: number;
  failed: number;
}

type Step = "loading" | "signed-out" | "request-sent" | "signed-in" | "error";

export function CloudAccountPanel() {
  const [step, setStep] = useState<Step>("loading");
  const [user, setUser] = useState<User | null>(null);
  const [email, setEmail] = useState("");
  const [pasteToken, setPasteToken] = useState("");
  const [status, setStatus] = useState<UploaderStatus | null>(null);
  const [internal, setInternal] = useState(false);
  const [err, setErr] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  async function refresh() {
    try {
      const isInternal = await invoke<boolean>("cloud_is_internal_mode");
      setInternal(isInternal);
      const u = await invoke<User>("cloud_whoami");
      setUser(u);
      setStep("signed-in");
    } catch {
      setStep("signed-out");
      setUser(null);
    }
    try {
      const s = await invoke<UploaderStatus>("cloud_uploader_status");
      setStatus(s);
    } catch { /* ignore */ }
  }

  useEffect(() => {
    refresh();
    const iv = setInterval(() => {
      invoke<UploaderStatus>("cloud_uploader_status").then(setStatus).catch(() => {});
    }, 5000);
    return () => clearInterval(iv);
  }, []);

  async function requestSignin(e: React.FormEvent) {
    e.preventDefault();
    if (!email.trim()) return;
    setBusy(true); setErr(null);
    try {
      await invoke("cloud_request_signin", { email: email.trim().toLowerCase() });
      setStep("request-sent");
    } catch (e) {
      setErr(String(e));
      setStep("error");
    } finally { setBusy(false); }
  }

  async function pasteAndVerify() {
    const t = pasteToken.trim();
    if (!t) return;
    setBusy(true); setErr(null);
    try {
      const u = await invoke<User>("cloud_verify", { token: t });
      setUser(u);
      setStep("signed-in");
      setPasteToken("");
      // Audit fix V1#1: bootstrap a default project right after sign-in so the producer
      // hook in council_multi has somewhere to send traces. Without this, every trace 404s.
      try {
        await invoke<string>("cloud_bootstrap_default_project");
      } catch (e) {
        console.warn("bootstrap default project failed (will retry on next sign-in):", e);
      }
    } catch (e) {
      setErr(String(e));
    } finally { setBusy(false); }
  }

  async function signOut() {
    setBusy(true);
    try {
      await invoke("cloud_revoke");
      setUser(null);
      setStep("signed-out");
      refresh();
    } catch (e) {
      setErr(String(e));
    } finally { setBusy(false); }
  }

  const dotColor = !status ? "#9aa3ae" : status.state === 0 ? "#3b8a55" : status.state === 1 ? "var(--accent)" : "#c0392a";

  return (
    <div className="cloud-account">
      {internal && (
        <div className="hint" style={{ background: "#fff7e0", border: "1px solid #e8d28a", padding: "8px 12px", borderRadius: 6, fontSize: 12, marginBottom: 12 }}>
          🛠 <strong>Internal mode</strong> — auth flows skip the cloud and target your local backend. Magic links return inline.
        </div>
      )}

      {step === "loading" && <p style={{ color: "#9aa3ae", fontSize: 13 }}>Loading account state…</p>}

      {step === "signed-out" && (
        <div>
          <div style={{ fontSize: 12, color: "#5b6470", marginBottom: 14, lineHeight: 1.6 }}>
            Two ways to sign in: clic the <code>furx://</code> link in the email — Furx opens automatically (post-install bundle).
            Or use the <em>https://</em> link → copy the session token from the dashboard → paste below.
          </div>
          <form onSubmit={requestSignin} style={{ display: "flex", gap: 8, alignItems: "flex-end", marginBottom: 12 }}>
            <div style={{ flex: 1 }}>
              <label style={{ display: "block", fontSize: 11, color: "#5b6470", textTransform: "uppercase", letterSpacing: ".05em", marginBottom: 4 }}>Email</label>
              <input
                type="email" required value={email}
                onChange={(e) => setEmail(e.target.value)}
                placeholder="you@example.com"
                disabled={busy}
                style={{ width: "100%", padding: "8px 10px", border: "1px solid #d6d3c9", borderRadius: 6, fontSize: 13 }}
              />
            </div>
            <button type="submit" disabled={busy || !email.trim()} className="btn-primary">
              {busy ? "Sending…" : "Send magic link"}
            </button>
          </form>
          <p style={{ fontSize: 12, color: "#5b6470", marginTop: 8 }}>
            We email you a one-time link. No password ever. Until you sign in, all your data stays local — Furx works offline by design.
          </p>
        </div>
      )}

      {step === "request-sent" && (
        <div>
          <div style={{ background: "var(--accent-glow)", border: "1px solid var(--accent)", borderRadius: 6, padding: 12, marginBottom: 12 }}>
            <strong style={{ fontSize: 13 }}>Check your email.</strong>
            <p style={{ fontSize: 12, color: "#0e1014", margin: "4px 0 0" }}>
              Link sent to <code>{email}</code>. Open it in your browser; it lands on <code>app.furx.cloud</code> which shows your session token.
              Paste it here to finish:
            </p>
          </div>
          <textarea
            value={pasteToken}
            onChange={(e) => setPasteToken(e.target.value)}
            placeholder="paste furx_sk_..."
            rows={3}
            style={{ width: "100%", padding: "8px 10px", border: "1px solid #d6d3c9", borderRadius: 6, fontSize: 12, fontFamily: "JetBrains Mono, monospace" }}
          />
          <div style={{ display: "flex", gap: 8, marginTop: 8 }}>
            <button onClick={pasteAndVerify} disabled={busy || !pasteToken.trim()} className="btn-primary">
              {busy ? "Verifying…" : "Verify token"}
            </button>
            <button onClick={() => { setStep("signed-out"); setPasteToken(""); }} className="btn-secondary">Cancel</button>
          </div>
          {err && <p style={{ color: "#c0392a", fontSize: 12, marginTop: 8 }}>{err}</p>}
        </div>
      )}

      {step === "signed-in" && user && (
        <div>
          <div style={{ display: "flex", alignItems: "center", gap: 12, padding: 12, background: "#faf8f1", border: "1px solid #e8e6df", borderRadius: 6, marginBottom: 12 }}>
            <div>
              <div style={{ fontSize: 13, color: "#0e1014" }}><strong>{user.email}</strong></div>
              <div style={{ fontSize: 11, color: "#5b6470", fontFamily: "JetBrains Mono, monospace" }}>plan: {user.plan}</div>
            </div>
            <button onClick={signOut} disabled={busy} className="btn-secondary" style={{ marginLeft: "auto" }}>
              Sign out
            </button>
          </div>

          {status && (
            <div style={{ display: "flex", alignItems: "center", gap: 8, fontSize: 12, color: "#5b6470" }}>
              <span style={{ display: "inline-block", width: 8, height: 8, borderRadius: "50%", background: dotColor }} />
              cloud sync · <strong>{status.state_label}</strong>
              {(status.succeeded > 0 || status.failed > 0 || status.dropped > 0) && (
                <span style={{ marginLeft: 8, fontFamily: "JetBrains Mono, monospace" }}>
                  · {status.succeeded} sent · {status.failed} failed · {status.dropped} dropped
                </span>
              )}
            </div>
          )}

          <p style={{ fontSize: 12, color: "#5b6470", marginTop: 12 }}>
            Council calls now flow to <code>api.furx.cloud</code> when a project has <em>Cloud Traces</em> enabled.
            Local SQLite remains the source of truth.
          </p>

          <CloudSmokeTest />
        </div>
      )}

      {step === "error" && (
        <div>
          <p style={{ color: "#c0392a", fontSize: 13 }}>{err}</p>
          <button onClick={() => { setStep("signed-out"); setErr(null); }} className="btn-secondary">Try again</button>
        </div>
      )}
    </div>
  );
}

// Sprint #6 — smoke test button that closes the loop visually.
// Emits a synthetic trace via cloud_emit_test_trace, polls uploader status until
// the counter ticks (or 30s timeout), then surfaces an "Open dashboard" link.
function CloudSmokeTest() {
  const [phase, setPhase] = useState<"idle" | "emitting" | "waiting" | "ok" | "fail" | "cooldown">("idle");
  const [msg, setMsg] = useState<string | null>(null);
  const [cooldown, setCooldown] = useState<number>(0);

  // Audit V2 fix: client-side debounce — disable button for 12s after each fire so
  // the user can't spam R2/D1 writes. Server-side limit (10s) is the hard cap.
  function startCooldown() {
    setCooldown(12);
    const iv = setInterval(() => {
      setCooldown((n) => {
        if (n <= 1) { clearInterval(iv); setPhase((p) => (p === "cooldown" ? "idle" : p)); return 0; }
        return n - 1;
      });
    }, 1000);
  }

  async function fire() {
    if (phase !== "idle" && phase !== "ok" && phase !== "fail") return;
    setPhase("emitting"); setMsg(null);
    try {
      const before = await invoke<UploaderStatus>("cloud_uploader_status");
      const baseSent = before.succeeded;
      const baseFail = before.failed;
      await invoke<string>("cloud_emit_test_trace");
      setPhase("waiting");
      const t0 = Date.now();
      const tick = async () => {
        try {
          const s = await invoke<UploaderStatus>("cloud_uploader_status");
          if (s.succeeded > baseSent) {
            setPhase("ok");
            setMsg("Trace landed in api.furx.cloud — open the dashboard to confirm.");
            return;
          }
          if (s.failed > baseFail) {
            setPhase("fail");
            setMsg(`Upload failed (counter ticked: ${s.failed}). Check network / signed-in state.`);
            return;
          }
        } catch (e) { console.warn("status poll:", e); }
        if (Date.now() - t0 < 30000) {
          setTimeout(tick, 1500);
        } else {
          setPhase("fail");
          setMsg("Timed out after 30s — queue may be paused (check status dot).");
        }
      };
      tick();
      startCooldown();
    } catch (e) {
      setPhase("fail"); setMsg(String(e));
      startCooldown();
    }
  }

  return (
    <div style={{ marginTop: 16, padding: 12, background: "#fcfcfa", border: "1px dashed #d6d3c9", borderRadius: 6 }}>
      <div style={{ fontSize: 12, color: "#5b6470", marginBottom: 8 }}>
        <strong>Smoke test</strong> — sends a synthetic trace through the full chain (producer → uploader → API → D1).
        No provider call, no real prompt; safe to run any time.
      </div>
      {phase === "idle" && (
        <button onClick={fire} disabled={cooldown > 0} className="btn-secondary" style={{ fontSize: 12, opacity: cooldown > 0 ? 0.5 : 1 }}>
          {cooldown > 0 ? `Send test trace (wait ${cooldown}s)` : "Send test trace"}
        </button>
      )}
      {(phase === "emitting" || phase === "waiting") && (
        <div style={{ fontSize: 12, color: "#5b6470", fontFamily: "JetBrains Mono, monospace" }}>
          {phase === "emitting" ? "enqueuing…" : "waiting for uploader to drain…"}
        </div>
      )}
      {phase === "ok" && (
        <div style={{ fontSize: 12, color: "#1e5a2f" }}>
          ✓ {msg}{" "}
          <a href="https://app.furx.cloud/traces/" target="_blank" rel="noopener noreferrer" style={{ color: "var(--accent)", textDecoration: "underline", marginLeft: 6 }}>
            Open dashboard ↗
          </a>
        </div>
      )}
      {phase === "fail" && (
        <div style={{ fontSize: 12, color: "#a12d1c" }}>
          ✗ {msg}{" "}
          <button onClick={fire} disabled={cooldown > 0} className="btn-secondary" style={{ fontSize: 11, marginLeft: 8, opacity: cooldown > 0 ? 0.5 : 1 }}>
            {cooldown > 0 ? `Retry in ${cooldown}s` : "Retry"}
          </button>
        </div>
      )}
    </div>
  );
}
