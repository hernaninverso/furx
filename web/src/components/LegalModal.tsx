// LegalModal — visor reusable para EULA / Privacy / Terms / DPA / Licencias.
// Council 7-voces must-fix: la app cobraba EULA aceptada sin mostrar nunca el
// texto al user. Ahora todos los documentos legales son accesibles desde
// Settings → Legal y también desde el Wizard (al primer install).

import { Modal } from "./Modal";
import { Button } from "./Button";

interface Props {
  title: string;
  body: string;
  onClose: () => void;
}

export function LegalModal({ title, body, onClose }: Props) {
  return (
    <Modal title={title} maxWidth={720} onClose={onClose}>
      <pre
        style={{
          maxHeight: "60vh",
          overflowY: "auto",
          padding: 16,
          background: "var(--color-surface-2, var(--bg2))",
          border: "1px solid var(--color-line, var(--line))",
          borderRadius: "var(--radius-md, 6px)",
          fontFamily: "var(--font-mono)",
          fontSize: "var(--fs-sm, 12px)",
          lineHeight: "var(--lh-loose, 1.65)",
          color: "var(--color-text, var(--text))",
          whiteSpace: "pre-wrap",
          wordBreak: "break-word",
          margin: 0,
        }}
        aria-label={`Texto completo: ${title}`}
      >
        {body}
      </pre>
      <div className="wizard-actions" style={{ marginTop: 12 }}>
        <Button variant="primary" onClick={onClose}>Cerrar</Button>
      </div>
    </Modal>
  );
}
