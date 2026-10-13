//! The system clipboard for room codes (COPY CODE, Ctrl+V in JOIN BY CODE).
//! Text only; everything degrades to "not available" (the COPY button
//! hides) when the platform has no clipboard (headless, no X11 / Wayland).

use std::sync::Mutex;

/// Kept open: on X11 / Wayland the copied text lives as long as its owner.
static BOARD: Mutex<Option<Option<arboard::Clipboard>>> = Mutex::new(None);

fn with<T>(f: impl FnOnce(&mut arboard::Clipboard) -> Option<T>) -> Option<T> {
    let mut slot = BOARD.lock().ok()?;
    let board = slot.get_or_insert_with(|| match arboard::Clipboard::new() {
        Ok(b) => Some(b),
        Err(e) => {
            log::info!("clipboard: not available ({e})");
            None
        }
    });
    board.as_mut().and_then(f)
}

/// Whether there is a clipboard to copy to.
pub fn available() -> bool {
    with(|_| Some(())).is_some()
}

/// Put `text` on the clipboard; false when that failed.
pub fn copy(text: &str) -> bool {
    with(|b| b.set_text(text.to_string()).ok()).is_some()
}

/// The clipboard's text, if any.
pub fn paste() -> Option<String> {
    with(|b| b.get_text().ok())
}
