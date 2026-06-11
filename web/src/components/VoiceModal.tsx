// F19 — Voice modal: record 5s via sox, transcribe via whisper-cli, write to focused pane.

import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { Modal } from "./Modal";
import { Button } from "./Button";

interface WhisperCheck {
  whisper_cli: string | null;
  model_path: string | null;
  sox: string | null;
  ready: boolean;
  install_hint: string;
  // 021-voice-es — true cuando está el modelo inglés viejo pero falta el multilingüe configurado.
  needs_migration: boolean;
}

interface Props {
  focusedPaneId: string | null;
  onClose: () => void;
}

export function VoiceModal({ focusedPaneId, onClose }: Props) {
  const [check, setCheck] = useState<WhisperCheck | null>(null);
  const [busy, setBusy] = useState<null | "downloading" | "recording" | "transcribing">(null);
  const [text, setText] = useState<string>("");
  const [err, setErr] = useState<string | null>(null);
  // BLOQUE H · D — streaming progress {downloaded, total} emitted by the
  // backend so the user sees actual MB instead of a frozen "downloading…".
  const [dlProgress, setDlProgress] = useState<{ downloaded: number; total: number | null } | null>(null);

  useEffect(() => {
    invoke<WhisperCheck>("whisper_check").then(setCheck).catch(() => setCheck(null));
    let off: (() => void) | undefined;
    listen<{ downloaded: number; total: number | null }>("voice:download-progress", (ev) => {
      setDlProgress(ev.payload);
    }).then((u) => { off = u; });
    return () => { off?.(); };
  }, []);

  const downloadModel = async () => {
    setBusy("downloading"); setErr(null); setDlProgress(null);
    try {
      await invoke("voice_download_model");
      const fresh = await invoke<WhisperCheck>("whisper_check");
      setCheck(fresh);
    } catch (e) {
      setErr(String(e));
    } finally { setBusy(null); setDlProgress(null); }
  };

  const recordAndTranscribe = async () => {
    if (!check?.ready) { setErr("setup incomplete — instala sox/whisper-cli/model"); return; }
    // spec 001 US1 — voice-interrupt: starting to talk cuts any ongoing TTS at once.
    invoke("tts_stop").catch(() => {});
    setBusy("recording"); setErr(null); setText("");
    try {
      const capture = await invoke<{ path: string; bytes: number; seconds: number }>("voice_capture", { seconds: 5 });
      setBusy("transcribing");
      const r = await invoke<{ text: string; elapsed_ms: number }>("voice_transcribe", { audioPath: capture.path });
      setText(r.text);
    } catch (e) {
      setErr(String(e));
    } finally { setBusy(null); }
  };

  const sendToFocused = async () => {
    if (!focusedPaneId || !text.trim()) return;
    await invoke("pty_write", { paneId: focusedPaneId, data: text + "\n", actionId: null, correlationId: null }).catch(console.error);
    onClose();
  };

  return (
    <Modal title="Voz" subtitle="Dictado local · sox 16kHz → whisper.cpp multilingüe (español)" maxWidth={540} onClose={onClose}>
      {!check
        ? <div className="muted">checking…</div>
        : check.ready
        ? (
          <>
            <p className="muted">Ready. {focusedPaneId ? "Output va al pane focado." : "Sin pane focado — abrí uno primero."}</p>
            <div className="wizard-actions">
              <button onClick={onClose}>Cerrar</button>
              <Button variant="primary" disabled={!!busy} onClick={recordAndTranscribe}>
                {busy === "recording" ? "🎤 grabando 5s…" : busy === "transcribing" ? "transcribiendo…" : "🎤 Grabar 5s"}
              </Button>
            </div>
          </>
        )
        : (
          <>
            <p className="muted">{check.needs_migration ? "El modelo inglés viejo no entiende español — descargá el multilingüe." : "Falta configurar el dictado de voz."}</p>
            <pre style={{ background: "var(--bg2)", padding: 10, fontSize: 11, color: "var(--cyan)", whiteSpace: "pre-wrap", marginTop: 8 }}>{check.install_hint}</pre>
            {!check.model_path && check.whisper_cli && check.sox && (
              <>
                <div className="wizard-actions" style={{ marginTop: 10 }}>
                  <Button variant="primary" disabled={!!busy} onClick={downloadModel}>
                    {busy === "downloading"
                      ? dlProgress
                        ? `descargando · ${(dlProgress.downloaded / 1024 / 1024).toFixed(1)}/${dlProgress.total ? (dlProgress.total / 1024 / 1024).toFixed(1) : "?"} MB`
                        : "descargando modelo multilingüe…"
                      : "Descargar modelo multilingüe"}
                  </Button>
                </div>
                {busy === "downloading" && dlProgress && dlProgress.total != null && (
                  /* BLOQUE H · D — live progress bar from voice:download-progress. */
                  <progress
                    aria-label="Whisper model download progress"
                    value={dlProgress.downloaded}
                    max={dlProgress.total}
                    style={{ width: "100%", marginTop: 10, height: 8 }}
                  />
                )}
              </>
            )}
          </>
        )}
      {err && <div className="card-block info" style={{ borderLeftColor: "var(--red)", marginTop: 12 }}>error: {err}</div>}
      {text && (
        <div style={{ marginTop: 12 }}>
          <div className="muted" style={{ fontSize: 11, marginBottom: 6 }}>transcripción:</div>
          <pre style={{ background: "var(--bg2)", border: "1px solid var(--line)", borderRadius: 6, padding: 10, fontSize: 12, color: "var(--text)", whiteSpace: "pre-wrap" }}>{text}</pre>
          <div className="wizard-actions">
            <button onClick={() => setText("")}>Descartar</button>
            <Button variant="primary" disabled={!focusedPaneId} onClick={sendToFocused}>Send to pane</Button>
          </div>
        </div>
      )}
    </Modal>
  );
}
