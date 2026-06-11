// 019 F2 T020 — InlineEdit: edición en sitio de un valor (texto/textarea). Destilada de los inputs
// editables de OrchestrationBoard/AgentGallery/SettingsRegistry. Commit en Enter o blur; Escape
// cancela. La PERSISTENCIA la hace el consumidor en `onCommit` y DEBE ir por el `invoke` gobernado
// (T022: inline-edit es mutación → gate universal + audit). Tokens V3, dark+light.
import { useEffect, useRef, useState } from "react";
import { kitInput } from "./styles";

export function InlineEdit({
  value, onCommit, multiline, placeholder, ariaLabel, disabled, maxLen = 4096,
}: {
  value: string;
  /**
   * Persistir el nuevo valor. Sólo se llama si cambió.
   * INVARIANTE DE GOBIERNO (audit ronda 2 H3): inline-edit es una MUTACIÓN — el consumidor DEBE
   * cablear `onCommit` al `invoke` gobernado (web/src/lib/invoke.ts → pending_approval → approvalBus
   * → audit). NUNCA persistir vía un fetch/invoke directo que bypasee el gate. Ver QueuePanel.tsx.
   */
  onCommit: (next: string) => void;
  multiline?: boolean;
  placeholder?: string;
  ariaLabel?: string;
  disabled?: boolean;
  maxLen?: number;
}) {
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState(value);
  const ref = useRef<HTMLInputElement | HTMLTextAreaElement | null>(null);

  // re-sincroniza si el valor externo cambia mientras NO se está editando (no pisa lo que tipeás).
  useEffect(() => { if (!editing) setDraft(value); }, [value, editing]);
  useEffect(() => { if (editing) ref.current?.focus(); }, [editing]);

  const commit = () => {
    const next = draft.slice(0, maxLen);
    setEditing(false);
    if (next !== value) onCommit(next);
    else setDraft(value);
  };
  const cancel = () => { setDraft(value); setEditing(false); };

  if (!editing) {
    return (
      <button
        type="button"
        disabled={disabled}
        aria-label={ariaLabel ? `Editar ${ariaLabel}` : "Editar"}
        onClick={() => !disabled && setEditing(true)}
        style={{
          textAlign: "left", cursor: disabled ? "default" : "text", width: "100%",
          background: "transparent", border: "1px solid transparent", borderRadius: "var(--radius, 3px)",
          padding: "6px 8px", color: value ? "var(--ink, #1c1814)" : "var(--ink-3, #635849)",
          fontFamily: "var(--font-sans, sans-serif)", fontSize: 14,
        }}
      >
        {value || placeholder || "—"}
      </button>
    );
  }

  const common = {
    ref: ref as never,
    value: draft,
    "aria-label": ariaLabel,
    placeholder,
    maxLength: maxLen,
    style: kitInput,
    onChange: (e: { target: { value: string } }) => setDraft(e.target.value),
    onBlur: commit,
    onKeyDown: (e: React.KeyboardEvent) => {
      if (e.key === "Escape") { e.preventDefault(); cancel(); }
      else if (e.key === "Enter" && !multiline) { e.preventDefault(); commit(); }
      else if (e.key === "Enter" && multiline && (e.metaKey || e.ctrlKey)) { e.preventDefault(); commit(); }
    },
  };
  return multiline
    ? <textarea {...common} rows={3} />
    : <input {...common} />;
}
