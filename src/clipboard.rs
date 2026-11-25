//! The system clipboard, via copypasta.

use copypasta::{ClipboardContext, ClipboardProvider};

pub struct Clipboard {
    provider: Box<dyn ClipboardProvider>,
}

impl Clipboard {
    /// Connect to the clipboard. `None` if no clipboard is available.
    ///
    /// The X11 clipboard is used everywhere on Linux; XWayland makes it visible to Wayland
    /// apps. (copypasta's Wayland clipboard takes the window's display connection and was
    /// observed to crash on exit, so it is not used.)
    pub fn new() -> Option<Clipboard> {
        let provider = ClipboardContext::new().ok()?;
        Some(Clipboard { provider: Box::new(provider) })
    }

    pub fn get(&mut self) -> Option<String> {
        self.provider.get_contents().ok()
    }

    pub fn set(&mut self, text: String) {
        let _ = self.provider.set_contents(text);
    }
}
