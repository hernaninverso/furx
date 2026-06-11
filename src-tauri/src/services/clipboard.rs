// F12 — Clipboard reader (via arboard). Tauri command polls this periodically
// from the frontend (or on a button click). We do NOT auto-poll inside Rust
// — that would risk a worker leak per V4 council; instead the UI drives.

use anyhow::Result;

pub fn read() -> Result<Option<String>> {
    // arboard::Clipboard::new() can fail on headless setups; treat as no-clipboard.
    match arboard::Clipboard::new() {
        Ok(mut cb) => match cb.get_text() {
            Ok(s) => Ok(Some(s)),
            Err(arboard::Error::ContentNotAvailable) => Ok(None),
            Err(e) => Err(anyhow::anyhow!("arboard get_text: {}", e)),
        },
        Err(e) => Err(anyhow::anyhow!("arboard init: {}", e)),
    }
}
