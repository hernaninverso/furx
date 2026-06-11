use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StartListeningResponse {
  /// The final on-device transcript. Empty if nothing was recognized.
  pub transcript: String,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LiveActivityRequest {
  /// Short, non-sensitive status line (no pane content — F-IV).
  pub status: Option<String>,
  pub pane_id: Option<String>,
}
