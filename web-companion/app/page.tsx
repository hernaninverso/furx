// Furx Web Companion · landing
// 058 — migrado a la identidad "Atelier Terminal" (tinta cálida + coral + Fraunces).

export default function Page() {
  return (
    <main style={mainStyle}>
      <div style={markStyle}>
        <span style={markGlyph}>F</span>
      </div>
      <h1 style={{ fontFamily: "Fraunces,Georgia,serif", fontSize: 44, margin: 0, letterSpacing: "-0.025em", fontVariationSettings: '"opsz" 96,"WONK" 1,"SOFT" 0', fontWeight: 600 }}>
        Furx Companion
      </h1>
      <p style={{ color: "#9c917b", maxWidth: 640, lineHeight: 1.7, marginTop: 12 }}>
        Audit replay read-only + session scrubber + aprobar tool-calls — desde cualquier browser,
        sin terminal exec. Privacy-first: cloud sync opt-in, encriptado del lado del cliente.
      </p>

      <section style={{ marginTop: 36, maxWidth: 640 }}>
        <h2 style={{ color: "#f2e8d8", fontSize: 22, fontFamily: "Fraunces,Georgia,serif", fontWeight: 600, letterSpacing: "-0.02em" }}>Emparejá con tu desktop</h2>
        <p style={{ color: "#9c917b" }}>
          En tu Mac / Linux / Windows, abrí <strong style={{ color: "#f2e8d8" }}>Furx → Ajustes → Móvil</strong> y copiá
          el <code style={code}>install_id</code> + el <code style={code}>código de 8 chars</code> de pairing.
        </p>
        <PairForm />
      </section>

      <footer style={{ marginTop: 60, color: "#6f6553", fontSize: 12 }}>
        © 2026 INVERSO HUB S.R.L. · <a href="https://github.com/hernaninverso/furx" style={{ color: "#ff8a6e" }}>GitHub</a> ·
        Read-only · cloud sync opt-in · audit encriptado del lado del cliente.
      </footer>
    </main>
  );
}

function PairForm() {
  return (
    <form action="/api/pair" method="post" style={{ display: "flex", gap: 8, marginTop: 14 }}>
      <input name="install_id" placeholder="install_id" style={inputStyle} />
      <input name="code" placeholder="código" style={{ ...inputStyle, width: 110 }} />
      <button type="submit" style={btnStyle}>Emparejar</button>
    </form>
  );
}

const mainStyle: React.CSSProperties = {
  background:
    "radial-gradient(1200px 600px at 78% -8%, rgba(255,138,110,0.06), transparent 60%), #16130f",
  color: "#f2e8d8",
  minHeight: "100vh",
  padding: 60,
  fontFamily: "'Hanken Grotesk',-apple-system,'SF Pro Text',sans-serif",
};
const markStyle: React.CSSProperties = {
  width: 56,
  height: 56,
  borderRadius: 15,
  display: "grid",
  placeItems: "center",
  background: "linear-gradient(145deg,#272019,#1e1a15)",
  border: "1px solid rgba(239,231,216,0.12)",
  boxShadow: "inset 0 1px 0 rgba(255,255,255,.06), 0 4px 14px rgba(0,0,0,.4)",
  marginBottom: 18,
};
const markGlyph: React.CSSProperties = {
  fontFamily: "Fraunces,Georgia,serif",
  fontStyle: "italic",
  fontWeight: 600,
  fontSize: 34,
  color: "#ff8a6e",
  lineHeight: 1,
  fontVariationSettings: '"WONK" 1,"SOFT" 0',
};
const code: React.CSSProperties = {
  background: "rgba(255,138,110,.10)",
  padding: "1px 6px",
  borderRadius: 5,
  color: "#ff8a6e",
  fontFamily: "'Space Mono',SF Mono,monospace",
};
const inputStyle: React.CSSProperties = {
  flex: 1,
  padding: "9px 13px",
  borderRadius: 9,
  border: "1px solid #322b21",
  background: "#211c17",
  color: "#f2e8d8",
  fontFamily: "'Space Mono',SF Mono,monospace",
  fontSize: 13,
};
const btnStyle: React.CSSProperties = {
  padding: "9px 18px",
  borderRadius: 9,
  border: "none",
  background: "#ff8a6e",
  color: "#16130f",
  fontWeight: 700,
  cursor: "pointer",
};
