// 019 F2 T021 — FormFromSchema: form data-driven con las 3 capas SEPARADAS (FR-010).
//   • RENDERING       → este componente pinta los `schema.fields` (sólo data).
//   • VALIDATION      → `validate()` de `lib/kit/schemaForm.ts` (pura, centralizada, anti-injection).
//   • EXECUTION-POLICY → el componente NUNCA ejecuta; al submit valida y, si pasa, DELEGA en
//                        `executeWithPolicy(policy, validated)`. La policy tiene allow-list de
//                        comandos y un `runner` inyectado (en la app = `invoke` gobernado → audit +
//                        gate de aprobación). Input crudo del usuario nunca llega al runner.
// Tokens V3, dark+light. Sin "honest/honesto".
import { useState } from "react";
import {
  validate, executeWithPolicy,
  type ExecutionPolicy, type FormSchema, type FormValues,
} from "../../lib/kit/schemaForm";
import { kitInput, kitLbl, kitBtn } from "./styles";

export function FormFromSchema({
  schema, policy, initial, submitLabel = "Ejecutar", onDone, onError,
}: {
  schema: FormSchema;
  policy: ExecutionPolicy;
  initial?: FormValues;
  submitLabel?: string;
  onDone?: (result: unknown) => void;
  onError?: (msg: string) => void;
}) {
  const [values, setValues] = useState<FormValues>(initial ?? {});
  const [errors, setErrors] = useState<Record<string, string>>({});
  const [busy, setBusy] = useState(false);

  const set = (name: string, v: string | number | boolean | null) =>
    setValues((prev) => ({ ...prev, [name]: v }));

  const submit = async (e: React.FormEvent) => {
    e.preventDefault();
    // CAPA 2 — validar antes de cualquier ejecución.
    const res = validate(schema, values);
    setErrors(res.errors);
    if (!res.ok || !res.validated) return;
    // CAPA 3 — delegar en la policy (allow-list + runner gobernado). El form no ejecuta.
    setBusy(true);
    try {
      const out = await executeWithPolicy(policy, res.validated);
      onDone?.(out);
    } catch (err) {
      onError?.(String(err instanceof Error ? err.message : err));
    } finally {
      setBusy(false);
    }
  };

  return (
    <form onSubmit={submit} style={{ display: "flex", flexDirection: "column", gap: 12 }}>
      {schema.fields.map((f) => {
        const err = errors[f.name];
        const v = values[f.name];
        return (
          <div key={f.name} style={{ display: "flex", flexDirection: "column", gap: 3 }}>
            <label style={kitLbl} htmlFor={`ffs-${f.name}`}>
              {f.label}{f.required ? " *" : ""}
            </label>
            {f.kind === "select" ? (
              <select id={`ffs-${f.name}`} style={kitInput} value={String(v ?? "")} onChange={(e) => set(f.name, e.target.value)}>
                <option value="">—</option>
                {(f.options ?? []).map((o) => <option key={o.value} value={o.value}>{o.label}</option>)}
              </select>
            ) : f.kind === "boolean" ? (
              <label style={{ display: "flex", alignItems: "center", gap: 8, fontFamily: "var(--font-sans, sans-serif)", fontSize: 14, color: "var(--ink, #1c1814)" }}>
                <input id={`ffs-${f.name}`} type="checkbox" checked={v === true} onChange={(e) => set(f.name, e.target.checked)} />
                {f.placeholder ?? "Sí"}
              </label>
            ) : f.kind === "textarea" ? (
              <textarea id={`ffs-${f.name}`} style={kitInput} rows={3} value={String(v ?? "")} placeholder={f.placeholder} maxLength={f.maxLen ?? 4096} onChange={(e) => set(f.name, e.target.value)} />
            ) : (
              <input id={`ffs-${f.name}`} style={kitInput} type={f.kind === "number" ? "number" : "text"} value={String(v ?? "")} placeholder={f.placeholder} onChange={(e) => set(f.name, f.kind === "number" ? (e.target.value === "" ? null : Number(e.target.value)) : e.target.value)} />
            )}
            {err && <span style={{ color: "var(--err, #a8412c)", fontSize: 12, fontFamily: "var(--font-sans, sans-serif)" }}>{err}</span>}
          </div>
        );
      })}
      <div style={{ display: "flex", justifyContent: "flex-end" }}>
        <button type="submit" style={kitBtn("accent")} disabled={busy}>{busy ? "…" : submitLabel}</button>
      </div>
    </form>
  );
}
