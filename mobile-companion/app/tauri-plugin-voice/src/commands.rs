use tauri::{command, AppHandle, Runtime};

use crate::models::*;
use crate::Result;
use crate::VoiceExt;

/// Start on-device speech recognition. Resolves with the final transcript when
/// `stop_listening` is called (or the recognizer finalizes). iOS only.
#[command]
pub(crate) async fn start_listening<R: Runtime>(app: AppHandle<R>) -> Result<StartListeningResponse> {
  app.voice().start_listening()
}

/// End the current utterance so the pending `start_listening` resolves.
#[command]
pub(crate) async fn stop_listening<R: Runtime>(app: AppHandle<R>) -> Result<()> {
  app.voice().stop_listening()
}

/// Start/update the "Claude is waiting" Live Activity (iOS 16.1+).
#[command]
pub(crate) async fn start_live_activity<R: Runtime>(
  app: AppHandle<R>,
  payload: LiveActivityRequest,
) -> Result<()> {
  app.voice().start_live_activity(payload)
}

/// End the Live Activity.
#[command]
pub(crate) async fn stop_live_activity<R: Runtime>(app: AppHandle<R>) -> Result<()> {
  app.voice().stop_live_activity()
}
