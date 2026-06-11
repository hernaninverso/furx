const COMMANDS: &[&str] = &[
  "start_listening",
  "stop_listening",
  "start_live_activity",
  "stop_live_activity",
];

fn main() {
  tauri_plugin::Builder::new(COMMANDS)
    .ios_path("ios")
    .build();
}
