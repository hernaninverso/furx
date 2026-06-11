// G — Toast renderer. Pairs with useToast. Stacks at bottom-right, dismissable
// by click, autodismiss governed by useToast.

import type { ToastSpec, UseToastApi } from "../hooks/useToast";

interface Props {
  toasts: ToastSpec[];
  onDismiss: UseToastApi["dismiss"];
}

const KIND_STYLES: Record<ToastSpec["kind"], { bg: string; fg: string; role: "status" | "alert" }> = {
  success: { bg: "var(--green-dim)", fg: "var(--green)", role: "status" },
  error:   { bg: "var(--red-dim)",  fg: "var(--red)",  role: "alert"  },
  info:    { bg: "var(--cyan-soft)", fg: "var(--cyan)", role: "status" },
};

export function ToastStack({ toasts, onDismiss }: Props) {
  if (toasts.length === 0) return null;
  return (
    <div
      style={{
        position: "fixed",
        right: 16,
        bottom: 16,
        zIndex: 200,
        display: "flex",
        flexDirection: "column",
        gap: 8,
        maxWidth: 360,
        pointerEvents: "none",
      }}
    >
      {toasts.map((t) => {
        const style = KIND_STYLES[t.kind];
        // Codex audit LOW #4: dismissible toasts need keyboard affordance.
        // The visual is a button, the live-region announcement comes from the
        // wrapping div with role=status/alert (announcers ignore button text).
        return (
          <div
            key={t.id}
            role={style.role}
            aria-live={style.role === "alert" ? "assertive" : "polite"}
            aria-atomic="true"
            style={{ pointerEvents: "auto" }}
          >
            <button
              type="button"
              onClick={() => onDismiss(t.id)}
              aria-label={`Dismiss ${t.kind} notification: ${t.message}`}
              style={{
                pointerEvents: "auto",
                cursor: "pointer",
                background: style.bg,
                color: style.fg,
                border: `1px solid ${style.fg}`,
                borderRadius: 6,
                padding: "8px 12px",
                fontSize: "0.85rem",
                boxShadow: "0 4px 12px rgba(0,0,0,0.2)",
                fontFamily: "var(--mono, monospace)",
                lineHeight: 1.4,
                width: "100%",
                textAlign: "left",
                display: "block",
              }}
            >
              {t.message}
            </button>
          </div>
        );
      })}
    </div>
  );
}
