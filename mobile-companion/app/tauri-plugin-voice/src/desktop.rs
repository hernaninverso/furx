use serde::de::DeserializeOwned;
use tauri::{plugin::PluginApi, AppHandle, Runtime};

use crate::models::*;

pub fn init<R: Runtime, C: DeserializeOwned>(
  app: &AppHandle<R>,
  _api: PluginApi<R, C>,
) -> crate::Result<Voice<R>> {
  Ok(Voice(app.clone()))
}

/// Desktop stub — native voice is iOS-only (the desktop has its own input).
pub struct Voice<R: Runtime>(#[allow(dead_code)] AppHandle<R>);

impl<R: Runtime> Voice<R> {
  pub fn start_listening(&self) -> crate::Result<StartListeningResponse> {
    Err(crate::Error::Unsupported(
      "native voice is only available on iOS".into(),
    ))
  }

  pub fn stop_listening(&self) -> crate::Result<()> {
    Err(crate::Error::Unsupported(
      "native voice is only available on iOS".into(),
    ))
  }

  pub fn start_live_activity(&self, _payload: LiveActivityRequest) -> crate::Result<()> {
    Err(crate::Error::Unsupported(
      "Live Activities are only available on iOS".into(),
    ))
  }

  pub fn stop_live_activity(&self) -> crate::Result<()> {
    Err(crate::Error::Unsupported(
      "Live Activities are only available on iOS".into(),
    ))
  }
}
