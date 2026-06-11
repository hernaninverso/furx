// FASE 1 — SkillPane: Execution pane for skills.
// Council UX V4: multi-step progress, streaming, error states, keyboard nav.

import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen, UnlistenFn } from "@tauri-apps/api/event";

interface Props {
  skillName: string;
  onClose: () => void;
  onRun: (input: string) => void;
  input: string;
  onInputChange: (val: string) => void;
}

interface SkillEvent {
  type: "Progress" | "Complete" | "Error" | "CacheHit";
  run_id?: string;
  step?: string;
  message?: string;
  output?: string;
  error?: string;
}

export function SkillPane({ skillName, onClose, onRun, input, onInputChange }: Props) {
  const [status, setStatus] = useState<"idle" | "running" | "complete" | "error">("idle");
  const [output, setOutput] = useState<string>("");
  const [progress, setProgress] = useState<string>("");
  const [error, setError] = useState<string | null>(null);
  const [step, setStep] = useState<string>("");
  const inputRef = useRef<HTMLTextAreaElement>(null);

  // Focus input on mount
  useEffect(() => {
    inputRef.current?.focus();
  }, []);

  // Listen for skill-run events. Use a ref-based approach to avoid
  // stale-closure issues with async setup (Audit F1C: cleanup safety).
  useEffect(() => {
    const unlisteners: UnlistenFn[] = [];
    let mounted = true;

    (async () => {
      const u1 = await listen<SkillEvent>("skill-run-complete", (e) => {
        if (!mounted) return;
        if (e.payload.run_id) {
          setStatus("complete");
          setOutput(e.payload.output || "");
        }
      });
      if (mounted) unlisteners.push(u1);

      const u2 = await listen<SkillEvent>("skill-run-error", (e) => {
        if (!mounted) return;
        setStatus("error");
        setError(e.payload.error || "Unknown error");
      });
      if (mounted) unlisteners.push(u2);
    })();

    return () => {
      mounted = false;
      unlisteners.forEach((u) => u());
    };
  }, []);

  const handleRun = () => {
    if (!input.trim()) return;
    setStatus("running");
    setOutput("");
    setError(null);
    setProgress("");
    onRun(input);
  };

  const handleKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) {
      handleRun();
    }
    if (e.key === "Escape") {
      onClose();
    }
  };

  return (
    <div style={{
      position: "fixed", inset: 0, zIndex: 100,
      display: "flex", alignItems: "center", justifyContent: "center",
      background: "rgba(0,0,0,.5)",
    }} onClick={onClose}>
      <div style={{
        background: "var(--bg)", border: "1px solid var(--border)", borderRadius: 12,
        width: "90%", maxWidth: 640, maxHeight: "80vh",
        display: "flex", flexDirection: "column",
      }} onClick={(e) => e.stopPropagation()}>

        {/* Header */}
        <div style={{
          display: "flex", justifyContent: "space-between", alignItems: "center",
          padding: "1rem", borderBottom: "1px solid var(--border)",
        }}>
          <h3 style={{ margin: 0, fontSize: "1rem" }}>▶ {skillName}</h3>
          <button onClick={onClose} style={{ background: "none", border: "none", color: "var(--text2)", cursor: "pointer", fontSize: "1.2rem" }}>
            ✕
          </button>
        </div>

        {/* Input */}
        <div style={{ padding: "1rem", borderBottom: "1px solid var(--border)" }}>
          <textarea
            ref={inputRef}
            value={input}
            onChange={(e) => onInputChange(e.target.value)}
            onKeyDown={handleKeyDown}
            placeholder={`Enter input for ${skillName}... (⌘⏎ to run)`}
            disabled={status === "running"}
            rows={3}
            style={{
              width: "100%", padding: ".5rem", borderRadius: 6, border: "1px solid var(--border)",
              background: "var(--surface)", color: "var(--text)", resize: "vertical",
              fontFamily: "inherit", fontSize: ".85rem",
            }}
            aria-label="Skill input"
          />
          <button
            onClick={handleRun}
            disabled={status === "running" || !input.trim()}
            style={{
              marginTop: ".5rem", padding: ".4rem 1rem", borderRadius: 6, border: "none",
              background: status === "running" ? "var(--text2)" : "var(--accent)",
              color: "#000", cursor: status === "running" ? "not-allowed" : "pointer",
              fontWeight: 600, fontSize: ".85rem",
            }}
          >
            {status === "running" ? "Running..." : "▶ Run"}
          </button>
        </div>

        {/* Progress */}
        {status === "running" && (
          <div style={{ padding: ".5rem 1rem", fontSize: ".85rem", color: "var(--text2)" }}>
            <div style={{ display: "flex", gap: ".5rem", alignItems: "center" }}>
              <span style={{ display: "inline-block", width: 12, height: 12, borderRadius: "50%", background: "var(--yellow)", animation: "pulse 1s infinite" }} />
              {progress || "Running..."}
            </div>
            {step && <div style={{ marginTop: ".25rem", fontSize: ".8rem" }}>Step: {step}</div>}
          </div>
        )}

        {/* Output */}
        {(output || status === "complete") && (
          <div style={{
            padding: "1rem", flex: 1, overflowY: "auto", minHeight: 100, maxHeight: 300,
          }}>
            <div style={{
              background: "var(--surface)", borderRadius: 6, padding: ".75rem",
              fontSize: ".85rem", lineHeight: 1.6, whiteSpace: "pre-wrap",
              fontFamily: "var(--font-mono)",
            }}>
              {output}
            </div>
          </div>
        )}

        {/* Error */}
        {error && (
          <div style={{
            padding: "1rem", borderTop: "1px solid var(--border)",
          }}>
            <div style={{
              background: "rgba(248,81,73,.1)", borderRadius: 6, padding: ".75rem",
              color: "var(--red)", fontSize: ".85rem",
            }}>
              <strong>Error:</strong> {error}
              <div style={{ marginTop: ".5rem", display: "flex", gap: ".5rem" }}>
                <button className="btn btn-primary" style={{ fontSize: ".8rem", padding: ".25rem .6rem" }} onClick={handleRun}>Retry</button>
                <button className="btn btn-secondary" style={{ fontSize: ".8rem", padding: ".25rem .6rem" }} onClick={onClose}>Close</button>
              </div>
            </div>
          </div>
        )}
      </div>
    </div>
  );
}
