use tauri::{
  plugin::{Builder, TauriPlugin},
  Manager, Runtime,
};

pub use models::*;

#[cfg(desktop)]
mod desktop;
#[cfg(mobile)]
mod mobile;

mod commands;
mod error;
mod models;

pub use error::{Error, Result};

#[cfg(desktop)]
use desktop::Voice;
#[cfg(mobile)]
use mobile::Voice;

/// Extensions to [`tauri::App`], [`tauri::AppHandle`] and [`tauri::Window`] to access the voice APIs.
pub trait VoiceExt<R: Runtime> {
  fn voice(&self) -> &Voice<R>;
}

impl<R: Runtime, T: Manager<R>> crate::VoiceExt<R> for T {
  fn voice(&self) -> &Voice<R> {
    self.state::<Voice<R>>().inner()
  }
}

/// Initializes the plugin.
pub fn init<R: Runtime>() -> TauriPlugin<R> {
  Builder::new("voice")
    .invoke_handler(tauri::generate_handler![
      commands::start_listening,
      commands::stop_listening,
      commands::start_live_activity,
      commands::stop_live_activity
    ])
    .setup(|app, api| {
      #[cfg(mobile)]
      let voice = mobile::init(app, api)?;
      #[cfg(desktop)]
      let voice = desktop::init(app, api)?;
      app.manage(voice);
      Ok(())
    })
    .build()
}
