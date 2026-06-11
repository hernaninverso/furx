// web/src/components/MobilePairingQR.tsx — 065 · pairing por QR del companion (council v4).
//
// Muestra un QR que codifica un TOKEN EFÍMERO (no el secreto permanente, que nunca sale del Keychain).
// El companion lo escanea, canjea el token por el secreto vía el bridge, y queda vinculado. Countdown
// real basado en `exp_epoch`. Short-code de respaldo si el scan falla. El comando `mobile_pairing_qr_generate`
// es Credential → el `invoke` gateado pide aprobación al generar.
import { useCallback, useEffect, useState } from "react";
import { QRCodeSVG } from "qrcode.react";
import { listen } from "@tauri-apps/api/event";
import { invoke } from "../lib/invoke";

interface PairingQrData {
  uri: string;
  session_id: string;
  short_code: string;
  exp_epoch: number; // epoch Unix real (coordinado con el companion)
}

type Status = "idle" | "pending" | "completed" | "error";

export function MobilePairingQR() {
  const [data, setData] = useState<PairingQrData | null>(null);
  const [secsLeft, setSecsLeft] = useState(0);
  const [status, setStatus] = useState<Status>("idle");
  const [errorMsg, setErrorMsg] = useState<string | null>(null);

  const generate = useCallback(async () => {
    setErrorMsg(null);
    try {
      const d = await invoke<PairingQrData>("mobile_pairing_qr_generate");
      setData(d);
      setStatus("pending");
    } catch (e) {
      setErrorMsg(String(e));
      setStatus("error");
    }
  }, []);

  // Countdown basado en exp_epoch real (no en un setInterval que puede driftear).
  useEffect(() => {
    if (status !== "pending" || !data) return;
    const tick = () => {
      const secs = Math.max(0, data.exp_epoch - Math.floor(Date.now() / 1000));
      setSecsLeft(secs);
      if (secs === 0) setStatus("idle");
    };
    tick();
    const id = setInterval(tick, 1000);
    return () => clearInterval(id);
  }, [status, data]);

  // Evento del bridge: pairing completado (match por session_id).
  useEffect(() => {
    if (!data) return;
    const un = listen<{ session_id: string }>("mobile-pairing-done", (ev) => {
      if (ev.payload.session_id === data.session_id) setStatus("completed");
    });
    return () => {
      void un.then((f) => f());
    };
  }, [data]);

  // Poll de respaldo por si el evento llegó antes de montar el listener.
  useEffect(() => {
    if (!data || status !== "pending") return;
    invoke<string>("mobile_pairing_status", { sessionId: data.session_id })
      .then((s) => {
        if (s === "completed") setStatus("completed");
        else if (s === "expired") setStatus("idle");
      })
      .catch(() => {});
  }, [data, status]);

  if (status === "completed") {
    return (
      <div className="row-meta" style={{ marginBottom: 10, color: "var(--green, #2e9e6b)" }}>
        ✓ Dispositivo vinculado. Ya podés usar el companion.
        <button style={{ marginLeft: 10 }} onClick={generate}>
          Vincular otro
        </button>
      </div>
    );
  }

  if (status === "error") {
    return (
      <div className="row-meta" style={{ marginBottom: 10 }}>
        <span style={{ color: "var(--red, #c0392b)" }}>No se pudo generar el QR: {errorMsg}</span>{" "}
        <button onClick={generate}>Reintentar</button>
      </div>
    );
  }

  if (status === "idle") {
    return (
      <div className="row-meta" style={{ marginBottom: 10 }}>
        <button onClick={generate}>{data ? "Regenerar QR de pareo" : "Vincular dispositivo móvil (QR)"}</button>
      </div>
    );
  }

  // pending
  const urgent = secsLeft <= 20;
  return (
    <div style={{ display: "flex", flexDirection: "column", alignItems: "center", gap: 10, margin: "8px 0 14px" }}>
      <div style={{ background: "#fff", padding: 12, borderRadius: 10 }}>
        <QRCodeSVG value={data!.uri} size={220} level="M" marginSize={2} />
      </div>
      <div className="muted" style={urgent ? { color: "var(--red, #c0392b)", fontWeight: 600 } : undefined}>
        Escaneá desde el companion · expira en{" "}
        <span style={{ fontFamily: "var(--font-mono, monospace)" }}>{secsLeft}s</span>
      </div>
      {data?.short_code && (
        <div className="muted" style={{ textAlign: "center" }}>
          <div style={{ fontSize: 11, marginBottom: 2 }}>Si el scan falla, ingresá este código en el companion:</div>
          <code style={{ fontSize: 20, letterSpacing: 3 }}>{data.short_code}</code>
        </div>
      )}
      <button style={{ fontSize: 12 }} onClick={generate}>
        Regenerar
      </button>
    </div>
  );
}
