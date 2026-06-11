use serde::de::DeserializeOwned;
use tauri::{
  plugin::{PluginApi, PluginHandle},
  AppHandle, Runtime,
};

use crate::models::*;

#[cfg(target_os = "ios")]
tauri::ios_plugin_binding!(init_plugin_voice);

// initializes the Swift plugin class
pub fn init<R: Runtime, C: DeserializeOwned>(
  _app: &AppHandle<R>,
  api: PluginApi<R, C>,
) -> crate::Result<Voice<R>> {
  #[cfg(target_os = "ios")]
  let handle = api.register_ios_plugin(init_plugin_voice)?;
  // Android isn't a target for the companion; only iOS provides native voice.
  #[cfg(target_os = "android")]
  let handle = api.register_android_plugin("cloud.furx.companion", "VoicePlugin")?;
  Ok(Voice(handle))
}

/// Access to the native voice APIs (iOS SFSpeechRecognizer, on-device).
pub struct Voice<R: Runtime>(PluginHandle<R>);

impl<R: Runtime> Voice<R> {
  pub fn start_listening(&self) -> crate::Result<StartListeningResponse> {
    self
      .0
      .run_mobile_plugin("startListening", ())
      .map_err(Into::into)
  }

  pub fn stop_listening(&self) -> crate::Result<()> {
    self
      .0
      .run_mobile_plugin("stopListening", ())
      .map_err(Into::into)
  }

  pub fn start_live_activity(&self, payload: LiveActivityRequest) -> crate::Result<()> {
    self
      .0
      .run_mobile_plugin("startLiveActivity", payload)
      .map_err(Into::into)
  }

  pub fn stop_live_activity(&self) -> crate::Result<()> {
    self
      .0
      .run_mobile_plugin("stopLiveActivity", ())
      .map_err(Into::into)
  }
}
