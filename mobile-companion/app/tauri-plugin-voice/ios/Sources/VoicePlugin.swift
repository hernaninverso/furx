import ActivityKit
import AVFoundation
import Speech
import SwiftRs
import Tauri
import UIKit
import WebKit

class LiveActivityArgs: Decodable {
  let status: String?
  let paneId: String?
}

// On-device speech recognition (council V2: requiresOnDeviceRecognition = true,
// so audio NEVER leaves the phone — F-I BYOK / F-IV privacy). Flow: start_listening
// begins capture and resolves with the final transcript when stop_listening is
// called (which ends audio so the recognizer finalizes); the webview then sends
// that text as a signed pty_write.
//
// Lifecycle (Codex review): all recognition state is mutated ONLY on the main
// queue; `stop_listening` ends audio input but does NOT tear down the session —
// teardown happens in `finish` once the final result/error arrives, so the
// pending `start_listening` invoke is never lost to a premature session reset.
class VoicePlugin: Plugin {
  private let audioEngine = AVAudioEngine()
  private var recognitionRequest: SFSpeechAudioBufferRecognitionRequest?
  private var recognitionTask: SFSpeechRecognitionTask?
  private var resolved = false
  // Stored as Any? so the property itself isn't gated on iOS 16.1 (the type
  // Activity<…> is); cast inside availability blocks.
  private static var liveActivityBox: Any?
  private var tapInstalled = false
  // Serialización del feed de audio (audit codex — crash POST-grabación). El tap corre
  // en el audio render thread; sin serializar, un `request.append(buffer)` podía ejecutarse
  // TRAS o concurrente con `endAudio()` → assert interno de Speech NO atrapable por do/catch
  // = la app se cierra. `acceptingAudio` bloquea nuevos buffers antes de cerrar; `audioEnded`
  // hace `endAudio()` idempotente (stopListening Y teardown lo llamaban → doble endAudio).
  private let audioFeedQueue = DispatchQueue(label: "cloud.furx.voice.audio-feed")
  private var acceptingAudio = false
  private var audioEnded = false

  @objc public func startListening(_ invoke: Invoke) throws {
    SFSpeechRecognizer.requestAuthorization { authStatus in
      DispatchQueue.main.async {
        guard authStatus == .authorized else {
          invoke.reject("speech recognition not authorized")
          return
        }
        AVAudioSession.sharedInstance().requestRecordPermission { granted in
          DispatchQueue.main.async {
            guard granted else {
              invoke.reject("microphone permission denied")
              return
            }
            self.beginRecognition(invoke)
          }
        }
      }
    }
  }

  // Always invoked on the main queue.
  private func beginRecognition(_ invoke: Invoke) {
    resolved = false
    recognitionTask?.cancel()
    recognitionTask = nil

    // Validate the recognizer BEFORE activating the audio session, so an
    // unavailable recognizer never leaves a dangling active session.
    guard let recognizer = SFSpeechRecognizer(), recognizer.isAvailable else {
      invoke.reject("speech recognizer unavailable")
      return
    }
    // On-device recognition (privacy F-IV): si el idioma del dispositivo NO tiene modelo
    // on-device, `requiresOnDeviceRecognition = true` haría fallar el request mudo →
    // mensaje claro en vez de "no funciona". (No caemos al server para no mandar el audio.)
    guard recognizer.supportsOnDeviceRecognition else {
      invoke.reject("dictado on-device no disponible para tu idioma en este dispositivo")
      return
    }

    let request = SFSpeechAudioBufferRecognitionRequest()
    request.shouldReportPartialResults = false
    request.requiresOnDeviceRecognition = true  // audio stays on-device
    recognitionRequest = request

    do {
      let session = AVAudioSession.sharedInstance()
      try session.setCategory(.record, mode: .measurement, options: .duckOthers)
      try session.setActive(true, options: .notifyOthersOnDeactivation)

      let inputNode = audioEngine.inputNode

      // CRASH-GUARD (el "se colgó al hablar"): `installTap` con un formato inválido
      // (0 Hz / 0 canales) dispara un assert NATIVO de CoreAudio
      // ("required condition is false: IsFormatSampleRateAndChannelCountValid(format)")
      // que NO se atrapa con try/catch → mata la app. Pasa cuando el hardware de audio
      // todavía no quedó configurado tras setCategory/setActive. Validamos ANTES de tapear.
      let format = inputNode.outputFormat(forBus: 0)
      guard format.sampleRate > 0, format.channelCount > 0 else {
        teardown()
        invoke.reject("micrófono no disponible (formato de audio inválido) — probá de nuevo")
        return
      }

      recognitionTask = recognizer.recognitionTask(with: request) { [weak self] result, error in
        // Recognition callbacks aren't guaranteed on the main thread — confine
        // ALL state mutation to main to avoid resolve/teardown races.
        DispatchQueue.main.async {
          guard let self = self else { return }
          if let result = result, result.isFinal {
            self.finish(invoke, transcript: result.bestTranscription.formattedString, error: nil)
          } else if let error = error {
            self.finish(invoke, transcript: nil, error: error.localizedDescription)
          }
        }
      }

      // Abrir la ventana de aceptación de buffers ANTES de tapear.
      audioFeedQueue.sync {
        self.acceptingAudio = true
        self.audioEnded = false
      }
      // Defensivo: removeTap por si quedó un tap viejo (un 2º installTap en el mismo
      // bus sin remover el anterior también crashea con "nullptr == Tap()").
      inputNode.removeTap(onBus: 0)
      // El append se serializa en `audioFeedQueue` (async, para NO bloquear el render
      // thread) y se guarda con `acceptingAudio` → nunca se appendea tras `endAudio()`.
      // `weak request`: si el request se liberó, el buffer se descarta sin crashear.
      inputNode.installTap(onBus: 0, bufferSize: 1024, format: format) { [weak self, weak request] buffer, _ in
        guard let self = self, let request = request else { return }
        self.audioFeedQueue.async {
          guard self.acceptingAudio else { return }
          request.append(buffer)
        }
      }
      tapInstalled = true
      audioEngine.prepare()
      try audioEngine.start()
    } catch {
      // Any failure after partial setup → full teardown before rejecting.
      teardown()
      invoke.reject("failed to start recognition: \(error.localizedDescription)")
    }
  }

  // Resolve/reject exactly once, then tear down. Main queue only.
  private func finish(_ invoke: Invoke, transcript: String?, error: String?) {
    if !resolved {
      resolved = true
      if let t = transcript {
        invoke.resolve(["transcript": t])
      } else {
        invoke.reject("recognition error: \(error ?? "unknown")")
      }
    }
    teardown()
  }

  // End audio input so the recognizer finalizes. Does NOT deactivate the
  // session — that happens in `finish` when the final result arrives, so the
  // pending start invoke isn't lost to a premature reset.
  @objc public func stopListening(_ invoke: Invoke) throws {
    DispatchQueue.main.async {
      if self.audioEngine.isRunning {
        self.audioEngine.stop()
      }
      self.removeTap()
      self.endAudioOnce()
    }
    invoke.resolve()
  }

  // Marca fin de input UNA sola vez, serializado con el tap. Bloquea nuevos buffers
  // (acceptingAudio=false) y hace endAudio idempotente (audioEnded) → cierra la carrera
  // append/endAudio y el doble endAudio (audit codex).
  private func endAudioOnce() {
    audioFeedQueue.sync {
      acceptingAudio = false
      guard !audioEnded else { return }
      audioEnded = true
      recognitionRequest?.endAudio()
    }
  }

  // Idempotent full teardown. Main queue only.
  private func teardown() {
    if audioEngine.isRunning {
      audioEngine.stop()
    }
    removeTap()
    endAudioOnce() // idempotente: no vuelve a llamar endAudio si stopListening ya lo hizo
    recognitionRequest = nil
    recognitionTask = nil
    try? AVAudioSession.sharedInstance().setActive(false, options: .notifyOthersOnDeactivation)
  }

  private func removeTap() {
    if tapInstalled {
      audioEngine.inputNode.removeTap(onBus: 0)
      tapInstalled = false
    }
  }

  // ── Live Activity ("Claude is waiting for input") ──────────────────────────
  // Started/ended from the webview on pane Busy→Ready. The FurxWidgets extension
  // renders it on the Lock Screen / Dynamic Island. ActivityKit matches the
  // running activity to the widget by the (same-named) FurxActivityAttributes.
  @objc public func startLiveActivity(_ invoke: Invoke) throws {
    guard #available(iOS 16.1, *) else {
      invoke.reject("Live Activities require iOS 16.1+")
      return
    }
    let args = try? invoke.parseArgs(LiveActivityArgs.self)
    let status = args?.status ?? "Claude is waiting for input"
    let paneId = args?.paneId ?? "pane"
    guard ActivityAuthorizationInfo().areActivitiesEnabled else {
      invoke.reject("Live Activities are disabled in Settings")
      return
    }
    let state = FurxActivityAttributes.ContentState(status: status)
    if let current = Self.liveActivityBox as? Activity<FurxActivityAttributes> {
      if current.attributes.paneId == paneId {
        // Same pane still waiting → update in place.
        Task { await current.update(using: state); invoke.resolve() }
        return
      }
      // Different pane → end the stale one (paneId is immutable) before
      // requesting a fresh activity for the new pane (Codex review).
      Self.liveActivityBox = nil
      Task { await current.end(dismissalPolicy: .immediate) }
    }
    do {
      let activity = try Activity.request(
        attributes: FurxActivityAttributes(paneId: paneId),
        contentState: state,
        pushType: nil)
      Self.liveActivityBox = activity
      invoke.resolve()
    } catch {
      invoke.reject("start live activity: \(error.localizedDescription)")
    }
  }

  @objc public func stopLiveActivity(_ invoke: Invoke) throws {
    guard #available(iOS 16.1, *) else {
      invoke.resolve()
      return
    }
    if let current = Self.liveActivityBox as? Activity<FurxActivityAttributes> {
      Self.liveActivityBox = nil
      Task { await current.end(dismissalPolicy: .immediate); invoke.resolve() }
    } else {
      invoke.resolve()
    }
  }
}

@_cdecl("init_plugin_voice")
func initPlugin() -> Plugin {
  return VoicePlugin()
}
